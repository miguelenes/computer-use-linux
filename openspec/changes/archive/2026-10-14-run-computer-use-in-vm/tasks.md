# Tasks

## 1. VM CLI and packaged setup assets

- [x] 1.1 Add the `computer-use-linux vm` command group for `check`, `setup`, `start`, `status`, `console`, `stop`, `suspend`, `destroy`, and `optimize`, preserving the current `computer-use-linux setup` accessibility behavior; verify help/output and regression tests keep the two setup meanings separate.
- [x] 1.2 Package the version-matched Vagrantfile and guest-provisioning assets for Cargo-installed and npm-launched binaries, and require an explicit source checkout path when one cannot be resolved; verify the packaged CLI stages assets outside the repository and finds the intended fork checkout.
- [x] 1.3 Implement read-only `vm check` for supported Debian/Ubuntu-compatible apt hosts, KVM, Vagrant, libvirt, provider plugin, resources, and checkout state; verify mocked ready/missing states, current AnduinOS detection, and unsupported-host guidance without writes.
- [x] 1.4 Add CLI usage and command-reference documentation to `README.md`, including that `computer-use-linux setup` still enables AT-SPI; verify examples match the implemented help and do not imply VM use is the default.
- [x] 1.5 Run `cargo fmt --all -- --check`, `cargo check --locked --all-targets`, `cargo clippy --locked --all-targets -- -D warnings`, and `cargo test --locked --no-fail-fast` for the CLI and asset-packaging changes; verify all applicable Rust contribution gates pass.

## 2. Host provider installation and guest setup

- [x] 2.1 Implement confirmed host setup for the documented Debian/Ubuntu-compatible apt matrix: preview Vagrant source/version, QEMU/KVM and libvirt packages/services, plugin build dependencies, group changes, and `vagrant-libvirt` installation; verify mocked execution, declining the initial stage makes no changes, declining later stages preserves and reports completed changes, and failures have actionable remediation.
- [x] 2.2 Generate per-user VM settings and the libvirt Vagrant configuration with `qemu:///system`, pinned box/version, host-passthrough CPU, virtio devices, SPICE, and configurable checkout sync; verify `vagrant validate`, resource overrides, virtiofs/rsync mode reporting, and no provider fallback.
- [x] 2.3 Make `vm setup` display and confirm the pinned image download, then create/start/provision the guest with resumable idempotent stages; verify box identity/size is shown before download and declining the image stage preserves and reports earlier host setup, and rerunning after each simulated interruption resumes without duplicating host or guest state.
- [x] 2.4 Provision the GNOME Wayland guest dependencies and build this checkout; configure unattended graphical login and a first-login task that runs the existing installer without rebuilding and completes session-specific AT-SPI/extension/doctor setup; verify on a clean guest that the package phase and first graphical session reach doctor readiness without user login or terminal intervention.
- [x] 2.5 Document host prerequisites, supported apt host matrix, privilege prompts, logout/service restart cases, guest start/console/login, stop/suspend/destroy, sync mode, and recovery after partial setup in `README.md`; verify every command against the CLI and Vagrant configuration.
- [x] 2.6 Document guest MCP-host selection, stdio configuration, session requirements, and user-owned login/portal consent in `README.md` and update `npm/README.md` and `skills/computer-use-linux/SKILL.md` where applicable; sync the plugin skill mirror if changed and verify there is no forwarded network endpoint.
- [x] 2.7 Run `scripts/install_sh_test.sh` and Vagrant/provisioning script checks against the guest bootstrap changes; verify existing installer behavior and new provisioning remain idempotent.

## 3. Per-VM optimization and measured performance

- [x] 3.1 Implement hardware-aware CPU/memory defaults with a host reserve, usable GNOME memory floor, 8-vCPU/8-GiB caps, and explicit overrides; verify boundary cases refuse unsafe automatic starts and status reports selected values.
- [x] 3.2 Implement `vm optimize` as a VM-local profile preview/apply flow that requires confirmation for restart/recreation and reports the active profile; verify no host-wide governor, kernel, swap, or security settings are modified.
- [x] 3.3 Add a reproducible native-versus-guest primitive workload and report format for screenshot, accessibility-read, and input p50/p95, including host/guest resources and indicator setting; verify repeated runs produce comparable measurements and that the default atomic-tool path adds no Jev work.
- [x] 3.4 Document the optional `COMPUTER_USE_LINUX_INDICATOR=0` guest profile, its hidden action cue, optimization controls, and the requirement for measurements before speed claims; verify default indicator behavior and safety remain unchanged.
- [x] 3.5 Add opt-in Jev configuration and the `run_jev_task` MCP tool, gated by `COMPUTER_USE_LINUX_ENABLE_JEV=1` and `TYPESAFE_API_KEY`; verify it is absent by default, missing credentials do not break ordinary tools, and the key is used only for provider authentication and no decision content, diagnostic, or result exposes it.
- [x] 3.6 Implement the bounded Jev decision loop with fresh immutable server-built candidate IDs, caller action allowlists, batched operation/target decisions, caller-only literal typing payloads, a bounded redacted caller task description, minimal redacted accessibility context, and a hard step budget; verify stale/malformed/abstaining decisions dispatch no action and screenshots, credentials, environment data, and typing payloads are excluded from decision content, logs, and results.
- [x] 3.7 Implement a documented structured postcondition schema, deterministic server evaluation from fresh observations, and safe incomplete/failure results for Jev tasks; verify missing or unsupported predicates are rejected before any action, completion is never accepted solely from Jev's response, and reobservation occurs after each action and before success.
- [x] 3.8 Add a reproducible end-to-end guest benchmark comparing the same multi-step tasks through atomic tools and `run_jev_task`; report sample counts, task mix, Jev latency, p50/p95, and independently verified completion, and permit a speedup claim only at >=2x lower median latency with no p95 regression or completion-rate reduction.
- [x] 3.9 Document Jev's disabled-by-default configuration, credential handling, data sent, action bounds, failure behavior, and benchmark gate; verify the benchmark distinguishes end-to-end Jev results from VM primitive latency.
- [x] 3.10 Suppress the on-screen activity indicator for `run_jev_task` only; verify the suppression is task-local and does not change concurrent or subsequent atomic tool indicator behavior.

## 4. MCP, compatibility, and release verification

- [x] 4.1 Run applicable contribution and packaging checks: `cargo publish --dry-run --locked`, `npm pack --dry-run`, `scripts/check_plugins.sh` when the skill mirror changes, `scripts/mcp_safety_check.py`, and `agnix .`; verify packaged binaries retain VM assets, `run_jev_task` registration follows the opt-in gate, and existing MCP annotations remain unchanged.
- [x] 4.2 Run host setup on the named AnduinOS 2.0.4 Debian-compatible apt host with KVM/libvirt and boot the pinned Ubuntu 26.04 GNOME Wayland guest; capture provider/version, active sync mode, guest image/session, and `computer-use-linux doctor` output, and resolve reported readiness failures.
- [x] 4.3 From an MCP host in the guest's graphical session, verify stdio tool registration, read a disposable application's accessibility tree, capture a real screenshot, perform a reversible input action, read state back, and confirm the host desktop is unaffected; record portal prompts and results. Repeat with Jev explicitly enabled, verify bounded `run_jev_task` execution and independent completion checks, and confirm the atomic tools still work.
- [x] 4.4 Run both the measured native-versus-guest primitive comparison and the Jev-versus-atomic end-to-end task comparison; record separate p50/p95 results, verified completion, and tested resource/display/indicator profile, and verify all compatibility and performance claims name the exact tested host, guest, and GNOME Wayland session and pass their respective evidence gates.

## Workflow follow-up

- Archive the change after implementation and review requirements are satisfied.
- Verify the archived change and synced specifications.
