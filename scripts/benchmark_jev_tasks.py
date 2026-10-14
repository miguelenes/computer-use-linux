#!/usr/bin/env python3
"""Compare caller-scripted MCP actions with run_jev_task on identical GUI fixtures.

Fixture JSON format:
{
  "scenarios": [{
    "name": "open panel",
    "app_name": "Text Editor",
    "window_id": 42,
    "task": "Open the menu and select Preferences",
    "allowlist": ["click"],
    "reset_actions": [{"operation":"press_key","key":"Escape"}],
    "atomic_actions": [{"operation":"click","role":"Open Menu"}],
    "literal_text": null,
    "postcondition": {"kind":"element_present","role":"menu item","name":"Preferences"},
    "max_steps": 3
  }]
}
Reset actions run outside the measured task. Atomic and Jev runs require the
same fixture and independently read the postcondition from fresh AT-SPI state.
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import sys
import time
from collections import Counter
from pathlib import Path
from typing import Any

from benchmark_primitives import McpClient, host_resources


WINDOW_TARGET_KEYS = ("window_id", "pid", "app_id", "wm_class", "title")


def window_target(scenario: dict[str, Any], *, click: bool = False) -> dict[str, Any]:
    target = {key: scenario[key] for key in WINDOW_TARGET_KEYS if scenario.get(key) is not None}
    if click and "title" in target:
        target["window_title"] = target.pop("title")
    return target


def percentile(values: list[float], quantile: float) -> float:
    ordered = sorted(values)
    return ordered[max(0, min(len(ordered) - 1, int(len(ordered) * quantile + 0.999999) - 1))]


def latency_summary(values: list[float]) -> dict[str, float]:
    return {
        "p50_ms": round(statistics.median(values), 3),
        "p95_ms": round(percentile(values, 0.95), 3),
        "mean_ms": round(statistics.fmean(values), 3),
    }


def structured(result: dict[str, Any]) -> dict[str, Any]:
    content = result.get("content", [])
    for block in content:
        if block.get("type") == "text":
            try:
                value = json.loads(block["text"])
            except (ValueError, KeyError):
                continue
            if isinstance(value, dict):
                return value
    value = result.get("structuredContent")
    return value if isinstance(value, dict) else {}


def get_state(client: McpClient, scenario: dict[str, Any]) -> dict[str, Any]:
    result = client.tool(
        "get_app_state",
        {
            "app_name_or_bundle_identifier": scenario["app_name"],
            "include_screenshot": False,
            "max_nodes": 600,
            "max_depth": 24,
        },
    )
    output = structured(result)
    if not output.get("tree_scoped") or output.get("accessibility_tree_truncated"):
        raise RuntimeError("fresh scoped accessibility state is incomplete")
    return output


def predicate_matches(state: dict[str, Any], predicate: dict[str, Any]) -> bool:
    for node in state.get("accessibility_tree", []):
        if node.get("role") != predicate.get("role") or node.get("name") != predicate.get("name"):
            continue
        kind = predicate.get("kind")
        if kind == "element_present":
            return True
        if kind == "element_enabled":
            return "enabled" in node.get("states", [])
        if kind == "field_value_equals":
            expected = predicate.get("value")
            text_value = node.get("text", {}).get("content") if isinstance(node.get("text"), dict) else None
            value_value = node.get("value", {}).get("text") if isinstance(node.get("value"), dict) else None
            return expected == text_value or expected == value_value
        raise RuntimeError(f"unsupported benchmark postcondition: {kind}")
    return False


def run_action(client: McpClient, scenario: dict[str, Any], action: dict[str, Any]) -> None:
    operation = action.get("operation")
    arguments: dict[str, Any]
    if operation == "click":
        arguments = {key: action[key] for key in ("role", "name", "text", "states") if key in action}
        if not (arguments.get("role") or arguments.get("name") or arguments.get("text")):
            raise RuntimeError("atomic click fixture requires a semantic role, name, or text selector")
        arguments.update(window_target(scenario, click=True))
    elif operation == "type_text":
        if not isinstance(action.get("text"), str):
            raise RuntimeError("atomic type_text fixture requires literal text")
        target = window_target(scenario)
        if not target:
            raise RuntimeError("atomic type_text fixture requires a window_id or another input window selector")
        arguments = {**target, "text": action["text"]}
    elif operation == "press_key":
        if not isinstance(action.get("key"), str):
            raise RuntimeError("atomic press_key fixture requires a key")
        target = window_target(scenario)
        if not target:
            raise RuntimeError("atomic press_key fixture requires a window_id or another input window selector")
        arguments = {**target, "key": action["key"]}
    elif operation == "perform_action":
        arguments = {key: action[key] for key in ("role", "name", "text", "action", "states") if key in action}
    else:
        raise RuntimeError(f"unsupported atomic action in fixture: {operation}")
    result = structured(client.tool(operation, arguments))
    if result and result.get("ok") is False:
        raise RuntimeError(f"atomic {operation} action failed")


def run_atomic(client: McpClient, scenario: dict[str, Any]) -> tuple[bool, float]:
    for action in scenario.get("atomic_actions", []):
        run_action(client, scenario, action)
        get_state(client, scenario)
    verification = get_state(client, scenario)
    return predicate_matches(verification, scenario["postcondition"]), 0.0


def run_jev(client: McpClient, scenario: dict[str, Any]) -> tuple[bool, float, str | None]:
    args = {
        "task": scenario["task"],
        "app_name": scenario["app_name"],
        "action_allowlist": scenario["allowlist"],
        "postcondition": scenario["postcondition"],
        "max_steps": scenario.get("max_steps", 5),
    }
    if scenario.get("window_id") is not None:
        args["window_id"] = scenario["window_id"]
    if "literal_text" in scenario and scenario["literal_text"] is not None:
        args["literal_text"] = scenario["literal_text"]
    result = structured(client.tool("run_jev_task", args))
    jev_latency = float(result.get("jev_latency_ms", 0))
    verification = get_state(client, scenario)
    complete = bool(result.get("verified")) and predicate_matches(verification, scenario["postcondition"])
    failure = None if complete else str(result.get("message") or result.get("status") or "independent verification failed")[:240]
    return complete, jev_latency, failure


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fixture", type=Path, help="JSON task fixtures shared by both modes")
    parser.add_argument("--samples", type=int, default=10)
    parser.add_argument("--server", default="computer-use-linux", help="MCP executable inside the guest graphical session")
    parser.add_argument("--host-cpus", type=int, help="physical host CPU count when run inside the guest")
    parser.add_argument("--host-memory-mib", type=int, help="physical host RAM in MiB when run inside the guest")
    parser.add_argument("--display-profile", default="unspecified", help="display backend and resolution used for both modes")
    parser.add_argument("--jev-key-stdin", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if not 3 <= args.samples <= 200:
        parser.error("--samples must be between 3 and 200")
    fixture = json.loads(args.fixture.read_text())
    if not isinstance(fixture, dict):
        parser.error("fixture JSON must contain an object with a scenarios array")
    scenarios = fixture.get("scenarios")
    if not isinstance(scenarios, list) or len(scenarios) < 3:
        parser.error("fixture must contain at least three repeatable scenarios")
    required_scenario_keys = {"name", "app_name", "task", "allowlist", "atomic_actions", "postcondition"}
    names: set[str] = set()
    for index, scenario in enumerate(scenarios):
        if not isinstance(scenario, dict) or not required_scenario_keys <= scenario.keys():
            parser.error(f"scenario {index} must define: {', '.join(sorted(required_scenario_keys))}")
        if not isinstance(scenario["name"], str) or not scenario["name"].strip() or scenario["name"] in names:
            parser.error(f"scenario {index} name must be non-empty and unique")
        names.add(scenario["name"])
        if not isinstance(scenario["app_name"], str) or not scenario["app_name"].strip():
            parser.error(f"scenario {scenario['name']} needs a non-empty app_name")
        if scenario.get("window_id") is not None and (
            not isinstance(scenario["window_id"], int) or scenario["window_id"] <= 0
        ):
            parser.error(f"scenario {scenario['name']} window_id must be positive when provided")
        if scenario.get("pid") is not None and (not isinstance(scenario["pid"], int) or scenario["pid"] <= 0):
            parser.error(f"scenario {scenario['name']} pid must be positive when provided")
        for key in ("app_id", "wm_class", "title"):
            if scenario.get(key) is not None and (not isinstance(scenario[key], str) or not scenario[key].strip()):
                parser.error(f"scenario {scenario['name']} {key} must be a non-empty string when provided")
        if not isinstance(scenario["task"], str) or not scenario["task"].strip():
            parser.error(f"scenario {scenario['name']} needs a non-empty task")
        if not isinstance(scenario["allowlist"], list) or not scenario["allowlist"]:
            parser.error(f"scenario {scenario['name']} needs a non-empty action allowlist")
        if any(action not in {"click", "perform_action", "type_text"} for action in scenario["allowlist"]):
            parser.error(f"scenario {scenario['name']} allowlist must contain click, perform_action, or type_text")
        if not isinstance(scenario["atomic_actions"], list) or not scenario["atomic_actions"]:
            parser.error(f"scenario {scenario['name']} needs at least one atomic action")
        if any(not isinstance(action, dict) for action in scenario["atomic_actions"]):
            parser.error(f"scenario {scenario['name']} atomic actions must be objects")
        if any(action.get("operation") not in scenario["allowlist"] for action in scenario["atomic_actions"]):
            parser.error(f"scenario {scenario['name']} atomic actions must match its Jev allowlist")
        if any(action.get("operation") in {"type_text", "press_key"} for action in scenario["atomic_actions"]) and not window_target(scenario):
            parser.error(f"scenario {scenario['name']} keyboard actions need window_id or another input window selector")
        if not isinstance(scenario["postcondition"], dict) or scenario["postcondition"].get("kind") not in {"element_present", "element_enabled", "field_value_equals"}:
            parser.error(f"scenario {scenario['name']} needs a supported independent postcondition")
        if not all(isinstance(scenario["postcondition"].get(key), str) and scenario["postcondition"][key] for key in ("role", "name")):
            parser.error(f"scenario {scenario['name']} postcondition needs exact role and name strings")
        if scenario["postcondition"]["kind"] == "field_value_equals" and not isinstance(scenario["postcondition"].get("value"), str):
            parser.error(f"scenario {scenario['name']} field_value_equals postcondition needs an exact string value")
        if not isinstance(scenario.get("reset_actions", []), list) or any(not isinstance(action, dict) for action in scenario.get("reset_actions", [])):
            parser.error(f"scenario {scenario['name']} reset_actions must be a list of action objects")
        if not 1 <= scenario.get("max_steps", 5) <= 5:
            parser.error(f"scenario {scenario['name']} max_steps must be between 1 and 5")
        if len(scenario["atomic_actions"]) > scenario.get("max_steps", 5):
            parser.error(f"scenario {scenario['name']} has more atomic actions than its Jev step budget")

    jev_env = os.environ.copy()
    jev_env.pop("TYPESAFE_API_KEY", None)
    jev_env.pop("COMPUTER_USE_LINUX_ENABLE_JEV", None)
    if args.jev_key_stdin:
        key = sys.stdin.readline().rstrip("\r\n")
        if not key:
            parser.error("--jev-key-stdin requires one credential line on stdin")
        jev_env["TYPESAFE_API_KEY"] = key
        jev_env["COMPUTER_USE_LINUX_ENABLE_JEV"] = "1"
        del key
    atomic_env = os.environ.copy()
    atomic_env.pop("TYPESAFE_API_KEY", None)
    atomic_env.pop("COMPUTER_USE_LINUX_ENABLE_JEV", None)
    clients = {
        "atomic": McpClient([args.server, "mcp"], env=atomic_env),
        "jev": McpClient([args.server, "mcp"], env=jev_env),
    }
    observations: dict[str, list[float]] = {"atomic": [], "jev": []}
    jev_latencies: list[float] = []
    verified: dict[str, int] = {"atomic": 0, "jev": 0}
    counts: dict[str, int] = {"atomic": 0, "jev": 0}
    jev_failures: Counter[str] = Counter()
    try:
        tools = {mode: {item["name"] for item in client.request("tools/list").get("tools", [])} for mode, client in clients.items()}
        if "run_jev_task" not in tools["jev"]:
            raise RuntimeError("run_jev_task is not registered; start the guest MCP host with Jev explicitly enabled")
        for sample in range(args.samples):
            for scenario in scenarios:
                for mode in (("jev", "atomic") if sample % 2 else ("atomic", "jev")):
                    client = clients[mode]
                    # Semantic clicks are resolved from a fresh cached AT-SPI
                    # observation. Prime the app before reset_actions too, as
                    # each MCP client process starts with an empty cache.
                    get_state(client, scenario)
                    for action in scenario.get("reset_actions", []):
                        run_action(client, scenario, action)
                    precondition = scenario.get("precondition")
                    initial_state = get_state(client, scenario)
                    if precondition and not predicate_matches(initial_state, precondition):
                        raise RuntimeError(f"scenario {scenario['name']} reset did not restore its precondition")
                    started = time.perf_counter_ns()
                    try:
                        if mode == "atomic":
                            complete, provider_ms = run_atomic(client, scenario)
                            failure = None
                        else:
                            complete, provider_ms, failure = run_jev(client, scenario)
                    except RuntimeError:
                        complete, provider_ms = False, 0.0
                        failure = "benchmark tool error"
                    elapsed_ms = (time.perf_counter_ns() - started) / 1_000_000
                    observations[mode].append(elapsed_ms)
                    counts[mode] += 1
                    verified[mode] += int(complete)
                    if mode == "jev":
                        jev_latencies.append(provider_ms)
                        if failure:
                            jev_failures[failure] += 1
    finally:
        for client in clients.values():
            client.close()

    atomic = latency_summary(observations["atomic"])
    jev = latency_summary(observations["jev"])
    completion = {mode: verified[mode] / counts[mode] for mode in counts}
    gate = jev["p50_ms"] <= atomic["p50_ms"] / 2 and jev["p95_ms"] <= atomic["p95_ms"] and completion["jev"] >= completion["atomic"]
    resources = host_resources()
    resources["host_cpus"] = args.host_cpus or resources["cpu_count"]
    resources["host_memory_mib"] = args.host_memory_mib or resources["mem_total_mib"]
    report = {
        "benchmark": "computer-use-linux-jev-tasks-v1",
        "sample_count_per_mode": args.samples * len(scenarios),
        "scenario_count": len(scenarios),
        "task_mix": [scenario["name"] for scenario in scenarios],
        "resources": resources,
        "indicator_enabled_for_server": os.environ.get("COMPUTER_USE_LINUX_INDICATOR", "1").lower() not in {"0", "false", "off", "no"},
        "display_profile": args.display_profile,
        "atomic": {"latency": atomic, "verified_completed": verified["atomic"], "completion_rate": completion["atomic"]},
        "jev": {"latency": jev, "provider_latency": latency_summary(jev_latencies) if jev_latencies else None, "verified_completed": verified["jev"], "completion_rate": completion["jev"], "failure_reasons": dict(jev_failures)},
        "speedup_gate_passed": gate,
        "claim_speedup": bool(gate),
        "gate": "Jev p50 must be <= half atomic p50, Jev p95 must not regress, and independently verified completion must not decrease.",
        "note": "Both modes use the same task fixture. Reset actions run outside measured time. The postcondition is checked again through fresh AT-SPI state after each task.",
    }
    serialized = json.dumps(report, indent=2)
    if args.output:
        args.output.write_text(serialized + "\n")
    print(serialized)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, RuntimeError, KeyError, TypeError) as error:
        print(f"benchmark failed: {error}", file=sys.stderr)
        raise SystemExit(1)
