# vm-runtime Specification

## Purpose

Provides an opt-in CLI workflow for installing and configuring the host virtualization provider, operating a reproducible Linux desktop VM, and running this fork's MCP server inside its graphical guest session.

## Requirements

### Requirement: VM use is opt-in
The project SHALL leave its existing direct-host installation and MCP configuration as the default. VM setup and startup SHALL require explicit VM commands, and a VM failure SHALL NOT silently switch desktop control to the host session.

#### Scenario: Existing direct-host installation
- **WHEN** a user follows an existing non-VM installation path
- **THEN** the project continues to install and run against the current host desktop without requiring Vagrant or a virtual-machine provider

#### Scenario: User opts into the VM
- **WHEN** a user runs an explicit `computer-use-linux vm` setup or lifecycle command
- **THEN** only the VM workflow is used and prerequisite or provider failures are reported with actionable remediation

### Requirement: VM host setup is available through an explicit CLI
The CLI SHALL provide separate VM preflight, setup, lifecycle, and status commands without changing the existing `computer-use-linux setup` accessibility behavior. Host package, service, group, provider-plugin, and box-download changes SHALL be previewed and require user confirmation before each stage is applied. Declining a stage SHALL prevent that stage and subsequent stages from making changes, preserve earlier completed stages, and report their state and the resume procedure.

#### Scenario: Read-only host preflight
- **WHEN** a user runs `computer-use-linux vm check`
- **THEN** the CLI reports host virtualization, supported package-manager, Vagrant, libvirt, provider-plugin, resource, and project prerequisites without changing host or guest state

#### Scenario: Confirmed provider setup
- **WHEN** a user runs `computer-use-linux vm setup` on a supported Linux host and confirms the displayed setup plan
- **THEN** the CLI installs or configures the required Vagrant, QEMU/KVM, libvirt, and Vagrant libvirt-provider components, provisions the configured guest, and reports any required logout or service restart

#### Scenario: User declines host changes
- **WHEN** a user declines the initial host setup plan before any stage is applied
- **THEN** the CLI leaves package, service, group, plugin, and guest state unchanged and prints the commands needed to resume later

#### Scenario: User declines a later setup stage
- **WHEN** a user declines an image-download or other setup stage after earlier confirmed stages completed
- **THEN** the CLI makes no changes for the declined or subsequent stages, preserves and reports earlier completed stages, and prints the commands needed to resume later

#### Scenario: Unsupported host distribution
- **WHEN** the host is outside the VM setup CLI's supported Debian/Ubuntu-compatible apt matrix
- **THEN** the CLI makes no host changes and reports the detected host plus manual setup steps instead of selecting another provider

### Requirement: VM provisioning uses hardware-assisted virtualization
The documented Linux VM profile SHALL use Vagrant with KVM/libvirt and hardware-assisted CPU virtualization. It SHALL provide configurable guest CPU and memory allocations and SHALL NOT claim a performance improvement without a reproducible measurement against a stated baseline.

#### Scenario: Supported Linux host
- **WHEN** a host has working KVM/libvirt, Vagrant, and the required Vagrant provider plugin
- **THEN** the VM runs through the libvirt provider with hardware-assisted virtualization

#### Scenario: Missing provider prerequisite
- **WHEN** Vagrant cannot use the required KVM/libvirt provider
- **THEN** startup reports the missing prerequisite and its installation or diagnostic command without silently selecting a slower provider

#### Scenario: Resource customization
- **WHEN** the user overrides the documented VM CPU or memory settings
- **THEN** Vagrant applies those values to the guest and preserves the documented defaults when no override is supplied

### Requirement: Guest is provisioned for a graphical desktop session
The provisioned guest SHALL run a supported GNOME Wayland desktop and include the runtime pieces needed to observe and control that guest session through accessibility, screenshots, window targeting, and input. On the dedicated Vagrant image it SHALL start the `vagrant` graphical session automatically and complete first-login setup without manual login or terminal commands. The provisioned VM SHALL keep the guest desktop and its applications separate from the host desktop.

#### Scenario: Guest desktop is ready
- **WHEN** the user starts the guest
- **THEN** GDM automatically starts the `vagrant` GNOME Wayland session, first-login setup runs unattended, and the guest presents a desktop ready for computer-use-linux

#### Scenario: Guest provisioning completes
- **WHEN** `computer-use-linux vm setup` successfully provisions the pinned image from a selected checkout
- **THEN** the guest contains the GNOME Wayland desktop, server build, and documented runtime dependencies, with session-specific setup left for graphical login

#### Scenario: Host desktop remains separate
- **WHEN** the user interacts with an application displayed by the VM
- **THEN** computer-use-linux actions affect the guest session and do not control host applications

#### Scenario: Other compositor compatibility
- **WHEN** the guest is changed to a compositor other than the provisioned GNOME Wayland session
- **THEN** documentation identifies that session as requiring separate setup and validation under the project's support matrix

### Requirement: MCP host and server run together in the guest
The VM setup SHALL guide the user to run a compatible MCP host in the guest graphical session and configure it to launch computer-use-linux over local stdio. It SHALL NOT expose an unauthenticated network MCP endpoint or depend on an SSH-launched server inheriting GUI session state.

#### Scenario: MCP client starts in the graphical guest session
- **WHEN** a compatible MCP host in the guest launches the configured computer-use-linux server
- **THEN** the server observes and controls the guest's active graphical session over the existing stdio transport

#### Scenario: MCP client configuration is prepared
- **WHEN** the user completes VM setup
- **THEN** the CLI provides the stdio command and session requirements for configuring the selected guest MCP host while leaving host installation, login, and credentials under the user's control

#### Scenario: MCP server is launched outside the guest session
- **WHEN** a user starts the server from a non-graphical SSH session without the guest's desktop session context
- **THEN** setup guidance explains that this does not establish access to the active GNOME Wayland desktop and directs the user to launch the MCP host within the guest session

### Requirement: VM lifecycle and data boundaries are clear
The VM CLI SHALL provide commands to start, inspect, open the console, stop, suspend, and deliberately destroy the guest, including that destroying it removes guest-local state. The workflow SHALL identify guest data and host resources and SHALL NOT require mounting the host home directory or sharing host desktop-control devices.

#### Scenario: User stops a running VM
- **WHEN** the user runs the stop or suspend VM command
- **THEN** Vagrant performs that lifecycle action without deleting guest-local data

#### Scenario: User destroys a VM
- **WHEN** the user runs the destroy VM command
- **THEN** guidance warns that guest-local VM state will be removed and requires confirmation before destruction

#### Scenario: Host data isolation
- **WHEN** the VM is provisioned using the documented defaults
- **THEN** host home-directory contents, the host D-Bus session, host input devices, and host desktop are not mounted or passed through to the guest

### Requirement: VM resource optimization is safe and measurable
The VM CLI SHALL choose conservative resource and device settings from host capacity, allow explicit user overrides, and expose optional per-VM tuning without weakening host security settings. Performance claims SHALL use a reproducible native-versus-guest workload and report measured results.

#### Scenario: Host-aware resource defaults
- **WHEN** a user configures the VM without resource overrides
- **THEN** the CLI selects CPU and memory values that leave documented capacity for the host desktop and reports the selected values

#### Scenario: User applies an optimization profile
- **WHEN** a user requests a per-VM optimization profile
- **THEN** the CLI previews and confirms the VM-local changes, leaves host-wide kernel and security settings unchanged, and reports how to compare the result with the baseline

#### Scenario: Performance comparison
- **WHEN** the user runs the documented native and VM workload
- **THEN** results report guest resources and p50/p95 for screenshots, accessibility reads, and input calls, and distinguish optional indicator settings

#### Scenario: Jev is considered for speed
- **WHEN** the project evaluates Jev for computer-use performance
- **THEN** Jev is used only by the optional high-level task-orchestration path, remains outside the screenshot, accessibility, and input primitive implementations, and is not described as faster without the required benchmark evidence

### Requirement: Jev task orchestration is optional, bounded, and verified
The MCP server SHALL offer an optional `run_jev_task` tool for bounded multi-step desktop subtasks. It SHALL be disabled by default and enabled only when `COMPUTER_USE_LINUX_ENABLE_JEV=1` and `TYPESAFE_API_KEY` are configured. When enabled, it SHALL use Jev only to select among fresh server-built candidate actions above the existing screenshot, accessibility, and input backends. Existing atomic MCP tools SHALL remain usable with their current behavior and safety constraints.

#### Scenario: Jev remains disabled by default
- **WHEN** the server starts without the Jev opt-in flag
- **THEN** `run_jev_task` is not exposed and existing MCP tools operate without Jev calls or Jev credentials

#### Scenario: Jev is explicitly enabled
- **WHEN** the user enables Jev and supplies its required credential
- **THEN** the MCP server exposes `run_jev_task` while keeping the ordinary atomic tools available, and reports configuration failures without printing credentials

#### Scenario: Jev task activity stays off-screen
- **WHEN** `run_jev_task` executes an action
- **THEN** it suppresses its own on-screen activity indicator for that task while ordinary atomic tools retain their configured indicator behavior

#### Scenario: Jev chooses an action from a fresh observation
- **WHEN** `run_jev_task` evaluates the next step
- **THEN** the server builds immutable candidate IDs from the current observation, constrains them to the caller's allowlist and server safety rules, and sends one decision request that selects both operation and target

#### Scenario: Task execution remains bounded and uses literal caller input
- **WHEN** a task requests multi-step interaction
- **THEN** the server enforces a maximum step budget, invalidates candidates after state changes, reobserves after each action, and uses only literal text supplied by the caller rather than Jev-generated text or commands

#### Scenario: Jev context is minimized
- **WHEN** the server requests a Jev decision
- **THEN** it sends only a bounded, redacted caller task description and the minimum scoped, redacted accessibility context needed for that decision; screenshot bytes, credentials, environment data, and caller-supplied typing payloads are excluded from decision content, logs, and results. The TypeSafe API key is used only for provider authentication

#### Scenario: Jev decision is uncertain, stale, malformed, or unavailable
- **WHEN** Jev abstains, times out, returns an invalid or stale candidate, or cannot be reached
- **THEN** the server takes no unvalidated action, stops or safely retries within the configured budget, and returns an incomplete or failed result without falling back to an unrestricted action

#### Scenario: Postcondition is validated before execution
- **WHEN** a caller requests a Jev task
- **THEN** the server requires a supported structured observable postcondition, validates it before any desktop action, and rejects missing or unsupported predicates without acting

#### Scenario: Completion is independently verified
- **WHEN** Jev reports that a task is complete
- **THEN** the server obtains a fresh observation and reports success only when a deterministic server evaluator verifies the caller-supplied structured postcondition independently of Jev's completion claim

#### Scenario: Performance claim is gated by end-to-end evidence
- **WHEN** the project claims Jev makes multi-step computer use faster
- **THEN** a reproducible comparison against the same task flow using atomic tools shows at least 2x lower median end-to-end task time, no p95 latency regression, and no reduction in independently verified completion; otherwise the result is reported without a speedup claim

### Requirement: VM readiness is verified in the named session
VM setup SHALL provide checks for the running GNOME Wayland session, computer-use-linux doctor readiness, MCP tool registration through the guest host, accessibility-tree access, screenshot capture, and input against a disposable guest application. It SHALL distinguish reported backend readiness from a successful screenshot and actual guest interaction.

#### Scenario: Readiness diagnostics
- **WHEN** a user follows the VM verification procedure
- **THEN** they can identify blockers separately for session discovery, accessibility, windows, input, and screenshot capture

#### Scenario: End-to-end guest smoke check
- **WHEN** a user performs the documented smoke check in the guest
- **THEN** a screenshot is captured and a reversible input action is verified against a disposable guest application

#### Scenario: Compatibility claim
- **WHEN** the VM feature reports its validation coverage
- **THEN** it names the tested host distribution, guest image, and GNOME Wayland session and makes no untested provider or compositor compatibility claim
