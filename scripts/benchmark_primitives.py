#!/usr/bin/env python3
"""Measure bounded screenshots, scoped accessibility, and a reversible AT-SPI toggle via stdio MCP."""

from __future__ import annotations

import argparse
import json
import os
import platform
import statistics
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


class McpClient:
    def __init__(self, command: list[str], env: dict[str, str] | None = None) -> None:
        self.process = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
            env=env,
        )
        self.next_id = 1
        self.request(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "computer-use-linux-benchmark", "version": "1"},
            },
        )
        self.notify("notifications/initialized")

    def notify(self, method: str) -> None:
        assert self.process.stdin is not None
        self.process.stdin.write(json.dumps({"jsonrpc": "2.0", "method": method}) + "\n")
        self.process.stdin.flush()

    def request(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        assert self.process.stdin is not None and self.process.stdout is not None
        request_id = self.next_id
        self.next_id += 1
        message: dict[str, Any] = {"jsonrpc": "2.0", "id": request_id, "method": method}
        if params is not None:
            message["params"] = params
        self.process.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
        self.process.stdin.flush()
        while line := self.process.stdout.readline():
            try:
                response = json.loads(line)
            except json.JSONDecodeError:
                continue
            if response.get("id") == request_id:
                if "error" in response:
                    raise RuntimeError(f"MCP {method} failed: {response['error'].get('message', 'unknown error')}")
                return response.get("result", {})
        raise RuntimeError(f"MCP server closed while waiting for {method}")

    def tool(self, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        result = self.request("tools/call", {"name": name, "arguments": arguments})
        if result.get("isError"):
            raise RuntimeError(f"MCP tool {name} returned an error")
        return result

    def close(self) -> None:
        if self.process.stdin:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            self.process.wait(timeout=3)


def structured(result: dict[str, Any]) -> dict[str, Any]:
    value = result.get("structuredContent")
    if isinstance(value, dict):
        return value
    for block in result.get("content", []):
        if block.get("type") == "text":
            try:
                parsed = json.loads(block["text"])
            except (ValueError, KeyError):
                continue
            if isinstance(parsed, dict):
                return parsed
    return {}


def percentile(values: list[float], quantile: float) -> float:
    ordered = sorted(values)
    index = max(0, min(len(ordered) - 1, int((len(ordered) * quantile + 0.999999) - 1)))
    return ordered[index]


def summarize(samples_ms: list[float]) -> dict[str, float]:
    return {
        "p50_ms": round(statistics.median(samples_ms), 3),
        "p95_ms": round(percentile(samples_ms, 0.95), 3),
        "mean_ms": round(statistics.fmean(samples_ms), 3),
    }


def host_resources() -> dict[str, Any]:
    mem_available_mib = None
    mem_total_mib = None
    try:
        for line in Path("/proc/meminfo").read_text().splitlines():
            if line.startswith("MemAvailable:"):
                mem_available_mib = int(line.split()[1]) // 1024
            elif line.startswith("MemTotal:"):
                mem_total_mib = int(line.split()[1]) // 1024
    except OSError:
        pass
    memory_limit_mib = None
    cpu_quota = None
    try:
        raw_limit = Path("/sys/fs/cgroup/memory.max").read_text().strip()
        if raw_limit != "max":
            memory_limit_mib = int(raw_limit) // (1024 * 1024)
    except (OSError, ValueError):
        pass
    try:
        cpu_quota = Path("/sys/fs/cgroup/cpu.max").read_text().strip()
    except OSError:
        pass
    return {
        "platform": platform.platform(),
        "machine": platform.machine(),
        "cpu_count": os.cpu_count(),
        "mem_available_mib": mem_available_mib,
        "mem_total_mib": mem_total_mib,
        "cgroup_memory_limit_mib": memory_limit_mib,
        "cgroup_cpu_quota": cpu_quota,
        "vm_product_name": Path("/sys/class/dmi/id/product_name").read_text().strip()
        if Path("/sys/class/dmi/id/product_name").is_file()
        else None,
    }


def focused_window_matches(client: McpClient, app: str) -> bool:
    focus = structured(client.tool("focused_window", {})).get("focused_window")
    if not isinstance(focus, dict):
        return False
    identity = " ".join(str(focus.get(key) or "") for key in ("app_id", "wm_class", "title")).lower()
    return app.lower() in identity


def named_toggle(state: dict[str, Any], name: str) -> dict[str, Any]:
    candidates = [
        node
        for node in state.get("accessibility_tree", [])
        if node.get("role") in {"button", "toggle button"}
        and node.get("name") == name
        and any(action.get("name", "").lower() == "click" for action in node.get("actions", []))
        and any(value in node.get("states", []) for value in ("enabled", "sensitive"))
    ]
    if not candidates:
        raise RuntimeError(f"fresh accessibility tree has no actionable toggle named {name!r}")
    candidates.sort(key=lambda node: (node.get("depth", 0), node.get("index", 0)))
    shallowest = candidates[0].get("depth", 0)
    candidates = [node for node in candidates if node.get("depth", 0) == shallowest]
    if len(candidates) != 1:
        raise RuntimeError(f"fresh accessibility tree has {len(candidates)} equally scoped toggles named {name!r}")
    return candidates[0]


def toggle_is_checked(node: dict[str, Any]) -> bool:
    return "checked" in node.get("states", [])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("native", "guest"), required=True)
    parser.add_argument("--app", required=True, help="AT-SPI application name for scoped reads")
    parser.add_argument("--window-id", type=int, help="window id from a fresh list_windows call")
    parser.add_argument("--window-title", help="optional exact window-title substring for targeting the Escape key")
    parser.add_argument("--screenshot-scope", choices=("window", "display"), default="window")
    parser.add_argument("--input-scope", choices=("target", "focused-app", "atspi-action"), default="target")
    parser.add_argument("--input-toggle-name", default="Main Menu", help="exact actionable toggle name for atspi-action input")
    parser.add_argument("--samples", type=int, default=20)
    parser.add_argument("--warmup", type=int, default=3)
    parser.add_argument("--screenshot-max-width", type=int, default=800)
    parser.add_argument("--screenshot-max-height", type=int, default=450)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--server", default="computer-use-linux", help="MCP server executable")
    parser.add_argument("--host-cpus", type=int, help="physical host CPU count, especially when run inside the guest")
    parser.add_argument("--host-memory-mib", type=int, help="physical host RAM in MiB, especially when run inside the guest")
    parser.add_argument("--display-profile", default="unspecified", help="display backend and resolution used for this run")
    args = parser.parse_args()
    if not 5 <= args.samples <= 1000 or not 0 <= args.warmup <= 100:
        parser.error("--samples must be 5..1000 and --warmup must be 0..100")
    if not 1 <= args.screenshot_max_width <= 1280 or not 1 <= args.screenshot_max_height <= 900:
        parser.error("screenshot maximum dimensions must be width 1..1280 and height 1..900")
    if args.window_id is not None and args.window_id <= 0:
        parser.error("--window-id must be a positive window id from list_windows")
    if args.screenshot_scope == "window" and args.window_id is None:
        parser.error("--screenshot-scope window requires --window-id")
    if args.input_scope == "target" and args.window_id is None:
        parser.error("--input-scope target requires --window-id")

    client = McpClient([args.server, "mcp"])
    try:
        tools = client.request("tools/list").get("tools", [])
        names = {tool.get("name") for tool in tools}
        required = {"screenshot", "get_app_state"}
        required.add("perform_action" if args.input_scope == "atspi-action" else "press_key")
        if args.input_scope == "focused-app":
            required.add("focused_window")
        if not required <= names:
            raise RuntimeError(f"MCP server lacks benchmark tools: {', '.join(sorted(required - names))}")

        window_target: dict[str, Any] = {"window_id": args.window_id} if args.window_id is not None else {}
        if args.window_title and args.window_id is not None:
            window_target["title"] = args.window_title
        input_target = window_target if args.input_scope == "target" else {}
        input_metric = "input_atspi_action" if args.input_scope == "atspi-action" else "input_escape"
        timings: dict[str, list[float]] = {"screenshot": [], "accessibility_read": [], input_metric: []}
        screenshot_details: dict[str, Any] = {}
        for index in range(args.warmup + args.samples):
            started = time.perf_counter_ns()
            screenshot_target = window_target if args.screenshot_scope == "window" else {}
            screenshot = structured(client.tool("screenshot", {**screenshot_target, "max_width": args.screenshot_max_width, "max_height": args.screenshot_max_height, "max_bytes": 1_000_000}))
            screenshot_details = {
                key: screenshot[key]
                for key in ("source", "width", "height", "coordinate_width", "coordinate_height", "scale", "resized", "format")
                if key in screenshot
            }
            screenshot_ms = (time.perf_counter_ns() - started) / 1_000_000

            started = time.perf_counter_ns()
            state_args: dict[str, Any] = {"app_name_or_bundle_identifier": args.app, "include_screenshot": False, "max_nodes": 500, "max_depth": 32}
            if args.window_id is not None:
                state_args["window_id"] = args.window_id
            state_result = client.tool("get_app_state", state_args)
            state = structured(state_result)
            if not state.get("tree_scoped") or state.get("accessibility_tree_truncated"):
                raise RuntimeError("fresh scoped accessibility state is incomplete")
            read_ms = (time.perf_counter_ns() - started) / 1_000_000

            started = time.perf_counter_ns()
            if args.input_scope == "focused-app" and not focused_window_matches(client, args.app):
                raise RuntimeError("fresh focused-window observation does not match the target app")
            if args.input_scope == "atspi-action":
                toggle = named_toggle(state, args.input_toggle_name)
                was_checked = toggle_is_checked(toggle)
                client.tool("perform_action", {"element_index": toggle["index"], "action": "Click"})
            else:
                client.tool("press_key", {**input_target, "key": "Escape"})
            input_ms = (time.perf_counter_ns() - started) / 1_000_000
            if args.input_scope == "focused-app" and not focused_window_matches(client, args.app):
                raise RuntimeError("target app lost focus after the Escape input")
            if args.input_scope == "atspi-action":
                readback = structured(client.tool("get_app_state", state_args))
                if not readback.get("tree_scoped") or readback.get("accessibility_tree_truncated"):
                    raise RuntimeError("fresh accessibility readback was incomplete after the reversible toggle")
                observed = named_toggle(readback, args.input_toggle_name)
                if toggle_is_checked(observed) == was_checked:
                    raise RuntimeError(f"fresh accessibility readback did not verify the reversible {args.input_toggle_name!r} toggle")
                client.tool("perform_action", {"element_index": observed["index"], "action": "Click"})
                restored = structured(client.tool("get_app_state", state_args))
                if not restored.get("tree_scoped") or restored.get("accessibility_tree_truncated"):
                    raise RuntimeError("fresh accessibility readback was incomplete after restoring the toggle")
                if toggle_is_checked(named_toggle(restored, args.input_toggle_name)) != was_checked:
                    raise RuntimeError(f"fresh accessibility readback did not verify restoration of {args.input_toggle_name!r}")

            if index >= args.warmup:
                timings["screenshot"].append(screenshot_ms)
                timings["accessibility_read"].append(read_ms)
                timings[input_metric].append(input_ms)

        resources = host_resources()
        resources["host_cpus"] = args.host_cpus or (resources["cpu_count"] if args.mode == "native" else None)
        resources["host_memory_mib"] = args.host_memory_mib or (resources["mem_total_mib"] if args.mode == "native" else None)
        report = {
            "benchmark": "computer-use-linux-primitives-v1",
            "mode": args.mode,
            "sample_count": args.samples,
            "warmup_count": args.warmup,
            "task_mix": [f"{args.screenshot_scope} screenshot", "scoped accessibility read", f"reversible AT-SPI {args.input_toggle_name!r} toggle" if args.input_scope == "atspi-action" else f"{args.input_scope} Escape keypress"],
            "target": {"app": args.app, "window_id": args.window_id},
            "screenshot_scope": args.screenshot_scope,
            "screenshot_capture": screenshot_details,
            "input_scope": args.input_scope,
            "screenshot_max_dimensions": {"width": args.screenshot_max_width, "height": args.screenshot_max_height},
            "resources": resources,
            "display_profile": args.display_profile,
            "indicator_enabled_for_server": os.environ.get("COMPUTER_USE_LINUX_INDICATOR", "1").lower() not in {"0", "false", "off", "no"},
            "latency": {name: summarize(samples) for name, samples in timings.items()},
            "note": "Run native and guest modes with the same app, window task, sample count, display profile, and indicator setting. This measures primitive latency only; it does not establish end-to-end task speedup.",
        }
        serialized = json.dumps(report, indent=2)
        if args.output:
            args.output.write_text(serialized + "\n")
        print(serialized)
    finally:
        client.close()
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, ValueError) as error:
        print(f"benchmark failed: {error}", file=sys.stderr)
        raise SystemExit(1)
