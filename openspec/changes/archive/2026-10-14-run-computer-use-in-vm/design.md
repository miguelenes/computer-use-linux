# Design

## Context

See `proposal.md` for motivation and `specs/vm-runtime/spec.md` for the observable contract. The existing `computer-use-linux setup` CLI command enables GNOME toolkit accessibility; it must keep that meaning. The existing `install.sh` installs this server, configures optional ydotool, enables AT-SPI, installs the GNOME extension when run in a graphical session, and runs `doctor`. It remains the guest-side server installer.

The current host is AnduinOS 2.0.4 (`ID_LIKE=debian`) with Vagrant 2.4.9, libvirt 12, QEMU 10.2, accessible `/dev/kvm`, and 24 logical CPUs. `virt-host-validate qemu` passed KVM/device checks with warnings for devices cgroup and secure guest support. The default libvirt NAT network is active, but `vagrant-libvirt` is not installed. The Vagrant Cloud metadata lists a libvirt artifact for `bento/ubuntu-26.04` version `202606.01.0`; it remains a candidate until the full GUI/MCP workflow passes in a booted guest. At inspection, 48 GiB RAM was installed and about 14 GiB was available, which can change before VM setup.

## Goals / Non-Goals

**Goals:**

- Give users one opt-in CLI flow that checks, installs, configures, starts, inspects, and safely tunes the VM provider and guest.
- Keep host-system changes visible and user-confirmed; make declining a setup stage prevent its changes while preserving and reporting earlier completed stages.
- Make setup available from the fork's installed CLI as well as its source checkout, with packaged Vagrant/provisioning assets.
- Fully provision the pinned guest and complete the graphical-session setup while keeping MCP client login, credentials, and portal consent in the user's hands.
- Adapt per-VM resources to the current host capacity, preserve explicit overrides, and measure performance before making speed claims.

**Non-Goals:**

- Changing the established `computer-use-linux setup` AT-SPI command or default direct-host install.
- Installing or authenticating Codex Desktop or another MCP host, or changing a user's client configuration without an explicit client and file choice.
- Adding a network MCP listener, mounting host home, or passing through host desktop-control devices.
- Changing global kernel, CPU-governor, security-mitigation, or host power settings as an optimization.
- Claiming a VM or Jev is faster without reproducible measurements, or adding Jev calls inside the screenshot, accessibility, or input primitive implementations.

## Decisions

### Add a separate `computer-use-linux vm` command group

Add VM subcommands under `computer-use-linux vm` so setup for a guest is not conflated with the existing `computer-use-linux setup` command. The command surface will include read-only `check`, confirmed `setup`, `start`, `status`, `console`, `stop`, `suspend`, confirmed `destroy`, and previewable per-VM `optimize` operations. Guest desktop readiness is proved by running the existing `computer-use-linux doctor` and MCP host inside the logged-in GNOME session; host-side Vagrant/SSH checks will never be represented as proof of GUI access.

Package the Vagrantfile, provisioning scripts, and documented defaults with the CLI so `cargo install` and the npm launcher can use the feature. `vm setup` stages those version-matched assets in the user's XDG data directory and requires a source checkout path (or an explicit project-root option) for the code to be shared and built in the guest. It stores provider/resource/sync preferences in an XDG user config file, not in the user's repository, home mount, or a credential store. Vagrant's `.vagrant` state remains local to the VM workspace and is ignored by Git.

Alternatives considered: a standalone `vm/setup.sh` is easy to bootstrap but unavailable as a command from packaged installs and duplicates argument handling; reusing `computer-use-linux setup` would break its current accessibility meaning and still would not provide a lifecycle interface.

### Preflight and install the host provider in explicit phases

`computer-use-linux vm check` is read-only and reports the detected OS/package manager, CPU virtualization and `/dev/kvm`, Vagrant version, libvirt/QEMU services and URI access, plugin availability, available resources, and checkout readiness. The first automated host-install matrix is Debian/Ubuntu-compatible apt hosts, including the current AnduinOS host. Other hosts receive diagnosis and concrete manual setup commands, but no guessed package installation or provider fallback until their package mappings are validated.

`computer-use-linux vm setup` first prints the exact package/service/group/plugin changes and image download it proposes. It prompts before applying them, invokes the system package manager with interactive elevation, installs a supported Vagrant release and required QEMU/KVM/libvirt plus plugin build dependencies, enables the appropriate libvirt socket/service, installs the tested `vagrant-libvirt` plugin, and verifies the `qemu:///system` provider. It reports group changes that require a new login rather than claiming they are active. Declining the initial host-install prompt makes no changes. Declining a later stage prevents that stage and subsequent stages from running; earlier confirmed changes remain in place and are reported with an idempotent resume procedure. It does not auto-reboot or run broad `sudo` shell strings. Provider, package, or service failures stop with the exact failed check and remediation; VirtualBox, software emulation, and remote/SSH MCP alternatives are not selected silently.

Before fetching the pinned box, show its identity and known download size and request confirmation. The host-install plan distinguishes software installation and guest-image download so users can accept or cancel each consequential stage. Repeated setup checks current state and skips already-satisfied changes.

### Provision the pinned guest from this fork

Use Vagrant/libvirt with `qemu:///system`, `host-passthrough` CPU, virtio disk/network, and SPICE graphics bound to loopback. Use virtiofs for only the source checkout when the provider/host combination supports it; allow explicit rsync mode and report that it is a sync snapshot. Do not share home directories, desktop buses, display sockets, input devices, or USB/PCI devices. Keep credentials, user applications, and the VM disk in the guest.

Use `bento/ubuntu-26.04` version `202606.01.0` as the first candidate guest. The guest provisioner installs the GNOME Wayland desktop, portals, AT-SPI, Rust/build libraries, and SPICE agent, then builds the project from the shared checkout. Package and build steps run automatically. This dedicated, disposable Vagrant guest enables GDM automatic login for its `vagrant` account so setup does not require the user to log in manually. It installs the GNOME extension system-wide and sets GNOME dconf defaults for AT-SPI and the enabled extension before GDM starts the first desktop session, so the first Shell process loads window targeting without a logout/restart. A delayed graphical-session task runs the existing installer without rebuilding, performs doctor, and completes user-scoped runtime setup, retrying on a later automatic login if doctor reports blockers. It never attempts to answer a RemoteDesktop portal prompt or authenticate a guest MCP client.

The selected MCP host runs in the guest's logged-in graphical session and starts `computer-use-linux mcp` over stdio. Setup emits the applicable command/config snippet and points to client-specific placement instructions. It does not forward an MCP port or write a client configuration file unless a future explicit client-scoped operation is designed and confirmed.

### Use safe, host-aware VM tuning

Keep machine tuning confined to the VM definition. The default profile uses hardware virtualization, host CPU passthrough, virtio I/O, the fastest supported checkout sync, and a documented virtual display. Automatic CPU and memory values use current host capacity, cap the initial allocation at 8 vCPUs/8 GiB, preserve a documented host reserve, and refuse an automatic start when the available-memory floor for a usable GNOME guest cannot be met. Users can override CPU, memory, display, and sync values explicitly. Persist only per-user VM configuration; report active settings in `vm status`.

`computer-use-linux vm optimize` compares the current settings with the selected safe profile, displays a VM-only diff, and requires confirmation before applying changes that need VM recreation/restart. Do not change host governor, hugepages, kernel parameters, security mitigations, or swap policy. Keep `COMPUTER_USE_LINUX_INDICATOR=0` an optional guest MCP-server setting that trades away the visible action cue; preserve the current default.

Provide a repeatable native-versus-guest workload for screenshots, accessibility reads, and input; record host/guest configuration and p50/p95 for each call. These primitive measurements guide VM profile choices and remain separate from end-to-end orchestration measurements.

### Add an opt-in Jev task orchestration layer

Add `run_jev_task` as a separate high-level MCP tool for bounded desktop subtasks. It is disabled unless `COMPUTER_USE_LINUX_ENABLE_JEV=1`; when enabled it requires `TYPESAFE_API_KEY`. Missing or invalid Jev configuration disables this optional path and reports a sanitized diagnostic without preventing startup or affecting existing atomic tools. The key is supplied through the guest MCP host's environment and is never returned in status, errors, or task output.

Keep Jev above the existing screenshot, AT-SPI, and input implementations. For each step, the server takes a fresh accessibility observation, constructs immutable candidate IDs and concrete action targets itself, filters those candidates through both server policy and a caller-provided action allowlist, and sends one Jev decision request for the operation and target together. The response may select a current candidate ID or abstain; it cannot supply arbitrary selectors, commands, or action arguments. Candidate IDs expire on any state change. The server enforces a small configured maximum step count and reobserves after every action. The caller may provide literal text to type, but the runner never asks Jev to generate or modify text and keeps those typing payloads out of the decision context.

Send Jev only a bounded, redacted task description supplied by the caller and the smallest relevant, redacted accessibility context. Exclude screenshot bytes, environment data, credentials, full desktop trees, and typing payloads from decision content, logs, and results. Use `TYPESAFE_API_KEY` only for provider authentication; never include it in model context. Preserve existing server safety checks and never treat the selected action as approval to bypass a primitive tool's constraints. If Jev abstains, is unavailable, or returns malformed, uncertain, or stale output, fail closed without dispatching an unvalidated action. Require the caller to supply a supported structured observable postcondition and validate it before any desktop action. Use a deterministic server evaluator over fresh accessibility observations, with documented predicates such as element presence, enabled state, or an exact field value within the task scope. Reject missing or unsupported predicates before acting. After the last action, take a fresh observation and report success only when that evaluator confirms the postcondition; Jev's own “done” response is not proof.

Evaluate the feature with repeated, reproducible multi-step tasks in the named guest, comparing end-to-end wall time and independently verified completion for the same tasks performed through atomic tools versus `run_jev_task`. Include Jev service latency in the measured result and report sample counts, task mix, resource profile, p50/p95, and completion rate. Claim a significant speedup only if median task time is at least 2x lower, p95 does not regress, and verified completion does not decrease. Otherwise keep the feature experimental and make no speedup claim. These results do not replace the separate screenshot/accessibility/input primitive comparison.

Research grounding: [Awesome Jev computer-use guide](https://github.com/AppitStudio/awesome-jev/blob/main/docs/computer-use.md) informs bounded candidates and fresh verification; [Cua's `jev-use` example](https://github.com/trycua/cua/blob/main/libs/cua-driver/examples/jev-use/README.md) informs candidate IDs, budgets, abstention, and independent verification; [TypeSafe speculative fan-out](https://docs.typesafe.ai/patterns/fan-out) informs batching related decisions in one request. These references motivate the design but do not establish compatibility with this Wayland guest or prove a speedup.

### Keep readiness and MCP verification in the graphical guest

The user opens the SPICE console, logs into GNOME Wayland, completes session setup, launches the MCP host there, and grants any portal prompt directly. The acceptance procedure captures host/provider check output, guest image and session versions, guest `computer-use-linux doctor`, MCP tool registration, a real screenshot, and a reversible action against a disposable guest application followed by a state read-back. A doctor report or a successful SSH command alone does not establish screenshot consent, session access, or input behavior.

Initial live validation is scoped to this AnduinOS Debian-compatible apt host and the pinned Ubuntu 26.04 GNOME Wayland guest. Other host distributions may receive read-only diagnostics/manual guidance but remain outside automated install compatibility until tested; other guest compositors, providers, and host MCP clients remain unclaimed until independently exercised.

## Risks / Trade-offs

- [Host package repositories or libvirt service names can differ even within Debian-compatible distributions] → Match OS identity to a validated adapter, show the exact package plan, test mocked adapter branches, and fail to manual instructions rather than guessing.
- [Installing libvirt/provider packages and changing group membership affects the host] → Keep `vm check` read-only, preview each install stage, require confirmation, use interactive elevation, explain logout requirements, and never reboot automatically.
- [The pinned Ubuntu image may change GNOME, portal, or provider behavior] → Run the named-session end-to-end acceptance before claiming support and update the pin only with a fresh validation.
- [Virtiofs may not work across every libvirt/plugin combination] → Verify capability in preflight, make rsync explicit, and show the active sync mode and staleness behavior.
- [Resource autodetection can be stale or the host can become busy] → Recheck before start, reserve host capacity, report the chosen values, allow overrides, and refuse below the documented guest-memory floor.
- [A writable source share exposes checkout files to the guest] → Share only the named project checkout and keep credentials and unrelated host state private.
- [VM graphical performance may be limited by the virtual display] → Measure the actual SPICE profile and report it with p50/p95 results; do not infer GPU acceleration.
- [Indicator-off mode removes visible action feedback] → Make it an opt-in guest profile, keep it off by default, and document the tradeoff.
- [Jev's remote decision latency or service availability can erase saved caller round trips] → Include provider latency in end-to-end benchmarks, keep atomic tools available, fail closed, and withhold speedup claims unless the stated gate passes.
- [Accessibility content may include private information] → Redact both the caller task description and scoped accessibility context; exclude screenshots, credentials, environment data, and caller typing payloads from decision content, logs, and results, and use the TypeSafe key only for provider authentication.

## Migration Plan

No migration is required. Existing host installations and the existing `computer-use-linux setup` command remain unchanged. Users choose `computer-use-linux vm setup` to install/configure the supported host provider and initialize a guest. `vm stop`/`vm suspend` preserve guest-local state; `vm destroy` requires a clear destructive confirmation and removes only the Vagrant guest, not host packages, provider plugins, or the source checkout. If a setup stage fails, the CLI reports the completed stages and an idempotent retry path rather than silently reverting unrelated host state.
