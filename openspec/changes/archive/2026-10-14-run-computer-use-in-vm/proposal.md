# Proposal

## Why

An isolated virtual desktop needs more than a Vagrantfile: users must be able to install and configure the host provider, provision the guest, tune its resources, and verify the live desktop without confusing VM setup with the existing AT-SPI `computer-use-linux setup` command. Multi-step computer-use tasks can also spend more time on repeated model/client round trips than on desktop primitives. This fork should offer an opt-in, bounded Jev orchestration path to reduce those round trips while keeping direct-host use and the existing atomic MCP tools unchanged.

## What Changes

- Add opt-in `computer-use-linux vm` commands for host preflight and setup, VM lifecycle, console access, status, and safe per-VM optimization.
- On supported Linux hosts, detect and preview required Vagrant, QEMU/KVM, libvirt, and Vagrant libvirt-provider changes; install/configure them only after explicit user confirmation, with actionable manual steps for unsupported systems.
- Provision the pinned GNOME Wayland guest from this checkout, guide the user through session-specific setup, and provide the stdio MCP configuration steps for a host running inside the guest.
- Select conservative hardware-aware VM resource defaults, allow explicit overrides, and provide reproducible native-versus-guest performance measurements without claiming the VM is faster absent evidence.
- Add an optional `run_jev_task` MCP tool that batches bounded multi-step desktop subtasks behind a feature flag, using Jev for high-level action selection while retaining the existing screenshot, accessibility, and input backends.
- Keep Jev disabled by default, require `TYPESAFE_API_KEY` only when enabled, preserve the existing direct-host install and `computer-use-linux setup` behavior, and leave portal consent and MCP-host login user-controlled.
- Require fresh server-built action candidates, caller allowlists, minimal redacted accessibility context, bounded steps, fail-closed handling, and independent state verification; make speedup claims only if an end-to-end benchmark meets the stated latency and completion gates.

## Capabilities

### New Capabilities

- `vm-runtime`: Opt-in host setup, operation, verification, and low-overhead tuning of an isolated Linux desktop VM running this fork's MCP server.

### Modified Capabilities

None.

## Impact

Adds nested VM-management commands to the Rust CLI, packaged Vagrant and guest-provisioning assets, host package/provider setup adapters, per-user VM configuration, an optional Jev-backed MCP orchestration tool, documentation, and setup/packaging validation. It affects host package-management and libvirt configuration only after explicit confirmation, and affects guest desktop, portal, accessibility, input, MCP, Jev configuration, and resource settings. Existing atomic MCP tools and their safety behavior remain available; the new tool is opt-in and does not install/authenticate an MCP client or change ordinary direct-host setup.
