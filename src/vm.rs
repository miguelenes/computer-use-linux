use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

const VAGRANTFILE: &str = include_str!("../assets/vm/Vagrantfile");
const PROVISION_SCRIPT: &str = include_str!("../assets/vm/provision.sh");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
struct VmSettings {
    checkout: PathBuf,
    box_name: String,
    box_version: String,
    cpus: u32,
    memory_mib: u32,
    sync_mode: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct VmOverrides {
    pub cpus: Option<u32>,
    pub memory_mib: Option<u32>,
    pub sync_mode: Option<String>,
}

pub(crate) fn resolve_checkout(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return validate_checkout(path);
    }
    if let Some(path) = std::env::var_os("COMPUTER_USE_LINUX_VM_CHECKOUT") {
        return validate_checkout(Path::new(&path));
    }
    let cwd = std::env::current_dir().context("could not determine the current directory")?;
    if is_checkout(&cwd) {
        return Ok(cwd);
    }
    bail!("source checkout not found; pass --checkout /path/to/computer-use-linux or set COMPUTER_USE_LINUX_VM_CHECKOUT")
}

fn validate_checkout(path: &Path) -> Result<PathBuf> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("source checkout does not exist: {}", path.display()))?;
    if !is_checkout(&canonical) {
        bail!(
            "{} is not a computer-use-linux source checkout (expected Cargo.toml and install.sh)",
            canonical.display()
        );
    }
    Ok(canonical)
}

fn is_checkout(path: &Path) -> bool {
    path.join("Cargo.toml").is_file()
        && path.join("install.sh").is_file()
        && std::fs::read_to_string(path.join("Cargo.toml"))
            .is_ok_and(|manifest| manifest.contains("name = \"computer-use-linux\""))
}

pub(crate) fn stage_assets(destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination).with_context(|| {
        format!(
            "could not create VM assets directory {}",
            destination.display()
        )
    })?;
    std::fs::write(destination.join("Vagrantfile"), VAGRANTFILE)
        .with_context(|| format!("could not stage Vagrantfile in {}", destination.display()))?;
    let provision_path = destination.join("provision.sh");
    std::fs::write(&provision_path, PROVISION_SCRIPT).with_context(|| {
        format!(
            "could not stage guest provisioner in {}",
            destination.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&provision_path, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("could not make {} executable", provision_path.display()))?;
    }
    Ok(())
}

pub(crate) fn user_vm_data_dir() -> Result<PathBuf> {
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(data_home).join("computer-use-linux/vm"));
    }
    let home = std::env::var_os("HOME")
        .context("HOME is not set; cannot locate the per-user VM workspace")?;
    Ok(PathBuf::from(home).join(".local/share/computer-use-linux/vm"))
}

pub(crate) fn run_setup(explicit_checkout: Option<&Path>, overrides: &VmOverrides) -> Result<()> {
    let checkout = resolve_checkout(explicit_checkout)?;
    let data_dir = user_vm_data_dir()?;
    let check = check(Some(&checkout))?;
    if check["host"]["supported"] != true {
        println!("{}", serde_json::to_string_pretty(&check)?);
        bail!("host is outside the automated Debian/Ubuntu-compatible apt setup matrix; follow the manual guidance from `vm check`");
    }

    let prior = user_vm_data_dir()
        .ok()
        .and_then(|dir| read_settings(&dir).ok());
    let (planned_cpus, planned_memory) = select_resources(
        overrides.cpus,
        overrides.memory_mib,
        prior
            .as_ref()
            .map(|settings| (settings.cpus, settings.memory_mib)),
    )?;
    validate_resources(planned_cpus, planned_memory)?;
    if overrides.sync_mode.as_deref() == Some("virtiofs") {
        resolve_sync_mode("virtiofs", virtiofsd_available(), true)?;
    }
    let vagrant = vagrant_plan()?;
    let mut packages = [
        // qemu-kvm is a virtual package on Ubuntu, and AnduinOS currently
        // publishes it without a candidate. Install the concrete system
        // emulator package that provides the KVM-backed QEMU binary.
        "qemu-system-x86",
        "ovmf",
        "libvirt-daemon-system",
        "libvirt-clients",
        "libvirt-dev",
        "virt-viewer",
        "ruby-dev",
        "pkg-config",
        "build-essential",
        "libxml2-dev",
        "libxslt1-dev",
        "zlib1g-dev",
        "rsync",
    ]
    .into_iter()
    .filter(|package| !package_installed(package))
    .map(str::to_string)
    .collect::<Vec<_>>();
    if !virtiofsd_available() && apt_package_candidate("virtiofsd") {
        packages.push("virtiofsd".into());
    }
    let package_list = packages.clone();
    let user = std::env::var("USER")
        .context("USER is not set; cannot configure libvirt group membership")?;
    let user_groups = command_output("id", &["-nG", &user]).unwrap_or_default();
    let active_groups = command_output("id", &["-nG"]).unwrap_or_default();
    let missing_groups = ["kvm", "libvirt"]
        .into_iter()
        .filter(|group| {
            !user_groups
                .split_whitespace()
                .any(|present| present == *group)
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    let login_refresh_needed = ["kvm", "libvirt"].into_iter().any(|group| {
        user_groups
            .split_whitespace()
            .any(|present| present == group)
            && !active_groups
                .split_whitespace()
                .any(|present| present == group)
    });
    let service_unit = Some(find_libvirt_service().unwrap_or_else(|| "libvirtd".into()));
    let service_needed = service_unit.as_ref().is_none_or(|unit| {
        command_output("systemctl", &["is-active", unit]).as_deref() != Some("active")
    });
    let plugin_needed = !provider_plugin_installed();

    println!("VM setup plan for {}:", checkout.display());
    println!(
        "  Vagrant: {}{} from {}",
        vagrant.version,
        if vagrant.install {
            " (install)"
        } else {
            " (already installed)"
        },
        vagrant.source
    );
    println!(
        "  apt packages: {}",
        if package_list.is_empty() {
            "already satisfied".into()
        } else {
            package_list.join(", ")
        }
    );
    println!(
        "  libvirt groups: {}",
        if missing_groups.is_empty() {
            "already satisfied".into()
        } else {
            missing_groups.join(", ")
        }
    );
    println!(
        "  libvirt service: {}",
        if service_needed {
            service_unit
                .as_deref()
                .unwrap_or("no recognized service unit")
        } else {
            "already active"
        }
    );
    println!(
        "  provider plugin: {}",
        if plugin_needed {
            "install vagrant-libvirt for the current user"
        } else {
            "already installed"
        }
    );
    println!(
        "  stage the pinned guest configuration in {}",
        data_dir.display()
    );
    println!(
        "  VM resources: {} vCPU, {} MiB RAM",
        planned_cpus, planned_memory
    );
    println!("  checkout sync: {} (auto uses virtiofs only when virtiofsd and vagrant-libvirt are available; otherwise rsync)", overrides.sync_mode.as_deref().unwrap_or("auto"));
    let resume = format!(
        "computer-use-linux vm setup --checkout {}",
        checkout.display()
    );
    let mut stages = vec![SetupStage {
        name: "plan",
        summary: "Apply the displayed host setup plan".into(),
    }];
    if vagrant.install {
        stages.push(SetupStage {
            name: "vagrant",
            summary: format!(
                "Install Vagrant {} from {}",
                vagrant.version, vagrant.source
            ),
        });
    }
    if !package_list.is_empty() {
        stages.push(SetupStage {
            name: "packages",
            summary: format!("Install apt packages: {}", package_list.join(", ")),
        });
    }
    if !missing_groups.is_empty() {
        stages.push(SetupStage {
            name: "groups",
            summary: format!("Add {user} to {}", missing_groups.join(", ")),
        });
    }
    if service_needed {
        stages.push(SetupStage {
            name: "service",
            summary: format!(
                "Enable and start {}",
                service_unit.as_deref().unwrap_or("libvirt service")
            ),
        });
    }
    if plugin_needed {
        stages.push(SetupStage {
            name: "plugin",
            summary: "Install the per-user vagrant-libvirt plugin".into(),
        });
    }
    stages.push(SetupStage {
        name: "vm-files",
        summary: format!(
            "Stage VM assets and checkout settings in {}",
            data_dir.display()
        ),
    });

    let mut completed = Vec::new();
    let result = execute_setup_stages(
        &stages,
        &mut completed,
        |stage| {
            println!("Setup stage: {}", stage.summary);
            confirm_stage(&format!("{}?", stage.summary))
        },
        |stage| match stage.name {
            "plan" => Ok(()),
            "vagrant" => install_vagrant(&vagrant),
            "packages" => install_packages(&package_list),
            "groups" => add_groups(&missing_groups, &user),
            "service" => enable_libvirt(service_unit.as_deref()),
            "plugin" => run_command("vagrant", &["plugin", "install", "vagrant-libvirt"], None),
            "vm-files" => stage_assets(&data_dir)
                .and_then(|()| write_settings(&data_dir, &checkout, overrides)),
            _ => unreachable!("stage list is built in this function"),
        },
    );
    if let Err(error) = result {
        report_setup_state(&completed, &resume);
        return Err(error).with_context(|| format!("setup did not finish; resume with `{resume}`"));
    }

    if let Err(error) = run_vagrant(&data_dir, &["validate"]) {
        report_setup_state(&completed, &resume);
        return Err(error).context(
            "Vagrant configuration validation failed; fix the provider issue and resume setup",
        );
    }

    let provider_ok = command_output("virsh", &["-c", "qemu:///system", "uri"]).as_deref()
        == Some("qemu:///system");
    if !provider_ok {
        bail!("Vagrant cannot access qemu:///system. Check `virsh -c qemu:///system uri`, libvirt service status, and log out/in after group changes.");
    }
    if login_refresh_needed || !missing_groups.is_empty() {
        println!("A new login session is required before the current shell receives updated libvirt group membership.");
    }

    println!(
        "VM assets and checkout settings are ready in {}",
        data_dir.display()
    );
    let settings = read_settings(&data_dir)?;
    println!(
        "Guest image: {} version {} (libvirt provider)",
        settings.box_name, settings.box_version
    );
    println!(
        "VM resources: {} vCPU, {} MiB RAM; checkout sync: {}",
        settings.cpus, settings.memory_mib, settings.sync_mode
    );
    let download_size = if box_is_installed(&settings)? {
        None
    } else {
        Some(box_download_size(&settings)?)
    };
    println!(
        "Guest image download: {}",
        download_size
            .map(format_bytes)
            .unwrap_or_else(|| "already cached locally".into())
    );
    if let Some(size) = download_size {
        if !confirm_stage(&format!(
            "Download {} version {} ({}) using the libvirt provider?",
            settings.box_name,
            settings.box_version,
            format_bytes(size)
        ))? {
            report_setup_state(&completed, &resume);
            bail!("guest image download declined; host setup is preserved")
        }
    }
    if download_size.is_some() {
        let version = settings.box_version.clone();
        run_vagrant(
            &data_dir,
            &[
                "box",
                "add",
                &settings.box_name,
                "--provider",
                "libvirt",
                "--box-version",
                &version,
            ],
        )?;
    }
    if !confirm_stage("Create, start, and provision the configured guest?")? {
        report_setup_state(&completed, &resume);
        bail!("guest creation declined; host setup and any downloaded image are preserved")
    }
    run_vagrant(&data_dir, &["up", "--provider", "libvirt", "--provision"])?;
    println!("Guest started and provisioned. Open its graphical console with `computer-use-linux vm console`.");
    println!("After the guest's next graphical boot, a one-shot desktop task completes AT-SPI, extension, and doctor setup automatically.");
    println!(
        "Rebooting the guest once so GDM loads the installed extension in a fresh Wayland session."
    );
    run_vagrant(&data_dir, &["reload"])?;
    Ok(())
}

struct SetupStage {
    name: &'static str,
    summary: String,
}

struct VagrantPlan {
    install: bool,
    version: String,
    source: String,
}

fn execute_setup_stages(
    stages: &[SetupStage],
    completed: &mut Vec<String>,
    mut confirm: impl FnMut(&SetupStage) -> Result<bool>,
    mut apply: impl FnMut(&SetupStage) -> Result<()>,
) -> Result<()> {
    for stage in stages {
        if !confirm(stage)? {
            bail!(
                "declined '{}' stage; completed stages: {}",
                stage.name,
                completed.join(", ")
            );
        }
        if let Err(error) = apply(stage) {
            bail!(
                "'{}' stage failed after completing [{}]: {error:#}",
                stage.name,
                completed.join(", ")
            );
        }
        completed.push(stage.name.to_string());
    }
    Ok(())
}

fn report_setup_state(completed: &[String], resume: &str) {
    println!(
        "Completed setup stages: {}",
        if completed.is_empty() {
            "none".into()
        } else {
            completed.join(", ")
        }
    );
    println!("Resume with: {resume}");
}

fn vagrant_plan() -> Result<VagrantPlan> {
    if let Some(version_line) = command_output("vagrant", &["--version"]) {
        let version = version_line
            .split_whitespace()
            .last()
            .unwrap_or_default()
            .to_string();
        if !supported_vagrant_version(&version) {
            bail!("Vagrant {version} is below the supported 2.4.0 minimum; update Vagrant from its official package source, then rerun `computer-use-linux vm check`");
        }
        let source = command_path("vagrant").unwrap_or_else(|| "PATH".into());
        return Ok(VagrantPlan {
            install: false,
            version,
            source,
        });
    }

    let policy = command_output("apt-cache", &["policy", "vagrant"])
        .context("Vagrant is missing and apt-cache could not find a Vagrant package; install a supported Vagrant 2.4+ release from its official apt source, then rerun `computer-use-linux vm check`")?;
    let candidate = policy
        .lines()
        .find_map(|line| line.trim().strip_prefix("Candidate:"))
        .map(str::trim)
        .filter(|version| !version.is_empty() && *version != "(none)")
        .context("Vagrant is missing and apt has no candidate; add the official Vagrant apt repository or install Vagrant 2.4+, then rerun `computer-use-linux vm check`")?;
    if !supported_vagrant_version(candidate) {
        bail!("apt candidate Vagrant {candidate} is below the supported 2.4.0 minimum; install a supported Vagrant 2.4+ release from its official apt source");
    }
    let source = policy
        .lines()
        .find(|line| {
            line.trim_start().starts_with("http://") || line.trim_start().starts_with("https://")
        })
        .map(str::trim)
        .unwrap_or("configured apt source")
        .to_string();
    Ok(VagrantPlan {
        install: true,
        version: candidate.to_string(),
        source,
    })
}

fn supported_vagrant_version(version: &str) -> bool {
    let mut parts = version.split('.').filter_map(|part| {
        part.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<u32>()
            .ok()
    });
    match (parts.next(), parts.next()) {
        (Some(major), Some(minor)) => (major, minor) >= (2, 4),
        _ => false,
    }
}

fn command_path(program: &str) -> Option<String> {
    let output = Command::new("sh")
        .args(["-c", "command -v -- \"$1\"", "sh", program])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn find_libvirt_service() -> Option<String> {
    ["libvirtd", "virtqemud"]
        .into_iter()
        .find(|unit| {
            command_output("systemctl", &["show", "-p", "LoadState", "--value", unit]).as_deref()
                == Some("loaded")
        })
        .map(str::to_string)
}

fn install_vagrant(plan: &VagrantPlan) -> Result<()> {
    if !plan.install {
        return Ok(());
    }
    run_sudo(&["apt-get", "update"])?;
    run_sudo(&[
        "apt-get",
        "install",
        "-y",
        &format!("vagrant={}", plan.version),
    ])
}

fn install_packages(packages: &[String]) -> Result<()> {
    if packages.is_empty() {
        return Ok(());
    }
    run_sudo(&["apt-get", "update"])?;
    let mut args = vec!["apt-get", "install", "-y"];
    args.extend(packages.iter().map(String::as_str));
    run_sudo(&args)
}

fn add_groups(groups: &[String], user: &str) -> Result<()> {
    for group in groups {
        run_sudo(&["usermod", "-aG", group, user])?;
    }
    println!("Group membership changed; log out and back in before Vagrant uses it.");
    Ok(())
}

fn enable_libvirt(unit: Option<&str>) -> Result<()> {
    let unit = unit.context("libvirt service unit was not found; install libvirt-daemon-system and inspect `systemctl list-unit-files '*libvirt*'`")?;
    run_sudo(&["systemctl", "enable", "--now", unit])
}

fn write_settings(data_dir: &Path, checkout: &Path, overrides: &VmOverrides) -> Result<()> {
    std::fs::create_dir_all(data_dir).with_context(|| {
        format!(
            "could not create VM settings directory {}",
            data_dir.display()
        )
    })?;
    let existing = read_settings(data_dir).ok();
    let effective_checkout = if checkout.join("Cargo.toml").is_file() {
        checkout
    } else {
        existing
            .as_ref()
            .map(|settings| settings.checkout.as_path())
            .unwrap_or(checkout)
    };
    let (cpus, memory_mib) = select_resources(
        overrides.cpus,
        overrides.memory_mib,
        existing
            .as_ref()
            .map(|settings| (settings.cpus, settings.memory_mib)),
    )?;
    validate_resources(cpus, memory_mib)?;
    let requested_sync = overrides
        .sync_mode
        .as_deref()
        .or_else(|| {
            existing
                .as_ref()
                .map(|settings| settings.sync_mode.as_str())
        })
        .unwrap_or("auto");
    let sync_mode = resolve_sync_mode(requested_sync, virtiofsd_available(), true)?;
    let settings = VmSettings {
        checkout: effective_checkout.to_path_buf(),
        box_name: "bento/ubuntu-26.04".into(),
        box_version: "202606.01.0".into(),
        cpus,
        memory_mib,
        sync_mode,
    };
    std::fs::write(
        data_dir.join("settings.json"),
        serde_json::to_vec_pretty(&settings)?,
    )
    .with_context(|| format!("could not write VM settings to {}", data_dir.display()))
}

fn validate_resources(cpus: u32, memory_mib: u32) -> Result<()> {
    if !(1..=8).contains(&cpus) {
        bail!("VM CPU override must be between 1 and 8 vCPUs");
    }
    if !(2048..=8192).contains(&memory_mib) {
        bail!("VM memory override must be between 2048 and 8192 MiB");
    }
    Ok(())
}

fn host_cpu_count() -> u32 {
    std::thread::available_parallelism()
        .map(|value| value.get().min(u32::MAX as usize) as u32)
        .unwrap_or(1)
}

fn host_memory_available_mib() -> Option<u32> {
    std::fs::read_to_string("/proc/meminfo")
        .ok()?
        .lines()
        .find(|line| line.starts_with("MemAvailable:"))?
        .split_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()
        .map(|kib| (kib / 1024).min(u32::MAX as u64) as u32)
}

fn default_resources(host_cpus: u32, available_memory_mib: u32) -> Result<(u32, u32)> {
    const HOST_CPU_RESERVE: u32 = 2;
    const HOST_MEMORY_RESERVE_MIB: u32 = 4096;
    const GUEST_MEMORY_FLOOR_MIB: u32 = 4096;
    let cpus = host_cpus.saturating_sub(HOST_CPU_RESERVE).min(8);
    let memory = available_memory_mib
        .saturating_sub(HOST_MEMORY_RESERVE_MIB)
        .min(8192)
        / 1024
        * 1024;
    if cpus == 0 || memory < GUEST_MEMORY_FLOOR_MIB {
        bail!("automatic VM start needs at least one guest vCPU and 4096 MiB guest memory while reserving {HOST_CPU_RESERVE} host CPUs and {HOST_MEMORY_RESERVE_MIB} MiB for the host; detected {host_cpus} CPUs and {available_memory_mib} MiB available. Free resources or pass explicit `--cpus` and `--memory-mib` values");
    }
    Ok((cpus, memory))
}

fn select_resources(
    explicit_cpus: Option<u32>,
    explicit_memory: Option<u32>,
    existing: Option<(u32, u32)>,
) -> Result<(u32, u32)> {
    let cpus = explicit_cpus.or_else(|| existing.map(|values| values.0));
    let memory = explicit_memory.or_else(|| existing.map(|values| values.1));
    if let (Some(cpus), Some(memory)) = (cpus, memory) {
        return Ok((cpus, memory));
    }
    let automatic = default_resources(host_cpu_count(), host_memory_available_mib().unwrap_or(0))?;
    Ok((cpus.unwrap_or(automatic.0), memory.unwrap_or(automatic.1)))
}

fn resolve_sync_mode(requested: &str, virtiofsd: bool, plugin_installed: bool) -> Result<String> {
    match requested {
        "auto" if virtiofsd && plugin_installed => Ok("virtiofs".into()),
        "auto" => Ok("rsync".into()),
        "rsync" => Ok("rsync".into()),
        "virtiofs" if virtiofsd && plugin_installed => Ok("virtiofs".into()),
        "virtiofs" => bail!("virtiofs was explicitly requested, but the host needs both virtiofsd and the vagrant-libvirt plugin; use `--sync rsync` or install the missing provider support"),
        other => bail!("unknown VM sync mode '{other}'; choose auto, virtiofs, or rsync"),
    }
}

fn read_settings(data_dir: &Path) -> Result<VmSettings> {
    let path = data_dir.join("settings.json");
    let contents = std::fs::read(&path).with_context(|| {
        format!(
            "VM settings are missing at {}; run `computer-use-linux vm setup`",
            path.display()
        )
    })?;
    let settings: VmSettings = serde_json::from_slice(&contents).with_context(|| {
        format!(
            "VM settings are invalid at {}; rerun `computer-use-linux vm setup`",
            path.display()
        )
    })?;
    validate_resources(settings.cpus, settings.memory_mib)?;
    Ok(settings)
}

fn box_is_installed(settings: &VmSettings) -> Result<bool> {
    let output = Command::new("vagrant")
        .args(["box", "list", "--machine-readable"])
        .output()
        .context("could not list locally cached Vagrant boxes")?;
    if !output.status.success() {
        bail!("`vagrant box list --machine-readable` failed; inspect Vagrant's output before continuing");
    }
    let listing = String::from_utf8_lossy(&output.stdout);
    Ok(box_list_contains(
        &listing,
        settings,
        vagrant_architecture(),
    ))
}

fn box_list_contains(listing: &str, settings: &VmSettings, expected_arch: &str) -> bool {
    let mut name = String::new();
    let mut provider = String::new();
    let mut version = String::new();
    for line in listing.lines() {
        let fields = line.splitn(4, ',').collect::<Vec<_>>();
        if fields.len() != 4 {
            continue;
        }
        match fields[2] {
            "box-name" => name = fields[3].into(),
            "box-provider" => provider = fields[3].into(),
            "box-version" => version = fields[3].into(),
            "box-architecture" => {
                if name == settings.box_name
                    && provider == "libvirt"
                    && version == settings.box_version
                    && fields[3] == expected_arch
                {
                    return true;
                }
                name.clear();
                provider.clear();
                version.clear();
            }
            _ => {}
        }
    }
    false
}

fn vagrant_architecture() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        _ => "amd64",
    }
}

fn box_download_size(settings: &VmSettings) -> Result<u64> {
    let url = format!(
        "https://vagrantcloud.com/{}/boxes/{}/versions/{}/providers/libvirt/{}/vagrant.box",
        settings.box_name.split('/').next().unwrap_or("bento"),
        settings
            .box_name
            .split('/')
            .nth(1)
            .unwrap_or("ubuntu-26.04"),
        settings.box_version,
        vagrant_architecture()
    );
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "-D",
            "-",
            "-o",
            "/dev/null",
            "-H",
            "Range: bytes=0-0",
            &url,
        ])
        .output()
        .context(
            "curl is required to look up the pinned Vagrant box size; no image was downloaded",
        )?;
    if !output.status.success() {
        bail!("could not read the pinned Vagrant box size from Vagrant Cloud; no image was downloaded. Check network access and retry `{}`", "computer-use-linux vm setup");
    }
    let headers = String::from_utf8_lossy(&output.stdout);
    let size = parse_range_total(&headers)
        .next_back()
        .context("Vagrant Cloud did not report a total box size; no image was downloaded");
    size
}

fn parse_range_total(headers: &str) -> impl DoubleEndedIterator<Item = u64> + '_ {
    headers.lines().filter_map(|line| {
        let (prefix, total) = line.split_once('/')?;
        prefix
            .to_ascii_lowercase()
            .contains("content-range:")
            .then(|| total.trim().parse().ok())?
    })
}

fn format_bytes(bytes: u64) -> String {
    format!(
        "{:.2} GiB ({} bytes)",
        bytes as f64 / 1024_f64.powi(3),
        bytes
    )
}

fn run_vagrant(data_dir: &Path, args: &[&str]) -> Result<()> {
    let settings = read_settings(data_dir)?;
    let status = Command::new("vagrant")
        .args(args)
        .current_dir(data_dir)
        .env("COMPUTER_USE_LINUX_VM_CHECKOUT", &settings.checkout)
        .env("COMPUTER_USE_LINUX_VM_CPUS", settings.cpus.to_string())
        .env(
            "COMPUTER_USE_LINUX_VM_MEMORY_MIB",
            settings.memory_mib.to_string(),
        )
        .env("COMPUTER_USE_LINUX_VM_SYNC", &settings.sync_mode)
        .env("VAGRANT_DEFAULT_PROVIDER", "libvirt")
        .status()
        .with_context(|| format!("could not run `vagrant {}`", args.join(" ")))?;
    if !status.success() {
        bail!("`vagrant {}` failed with {status}; provider is fixed to libvirt. Run `computer-use-linux vm check` and `vagrant validate` from {} for diagnostics", args.join(" "), data_dir.display());
    }
    Ok(())
}

fn confirm_stage(question: &str) -> Result<bool> {
    use std::io::{self, Write};
    print!("{question} [y/N] ");
    io::stdout()
        .flush()
        .context("could not display setup confirmation")?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .context("could not read setup confirmation")?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn package_installed(package: &str) -> bool {
    Command::new("dpkg-query")
        .args(["-W", "-f=${Status}", package])
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains("install ok installed")
        })
}

fn apt_package_candidate(package: &str) -> bool {
    command_output("apt-cache", &["policy", package]).is_some_and(|output| {
        output.lines().any(|line| {
            line.trim()
                .strip_prefix("Candidate:")
                .is_some_and(|candidate| {
                    !candidate.trim().is_empty() && candidate.trim() != "(none)"
                })
        })
    })
}

fn virtiofsd_available() -> bool {
    command_exists("virtiofsd")
        || ["/usr/libexec/virtiofsd", "/usr/lib/qemu/virtiofsd"]
            .iter()
            .any(|path| Path::new(path).is_file())
}

fn provider_plugin_installed() -> bool {
    command_output("vagrant", &["plugin", "list"]).is_some_and(|output| {
        output
            .lines()
            .any(|line| line.starts_with("vagrant-libvirt "))
    })
}

fn run_sudo(args: &[&str]) -> Result<()> {
    run_command("sudo", args, None)
}

fn run_command(program: &str, args: &[&str], cwd: Option<&Path>) -> Result<()> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let status = command
        .status()
        .with_context(|| format!("could not run {program} {}", args.join(" ")))?;
    if !status.success() {
        bail!("{program} {} failed with {status}; inspect the command output and rerun `computer-use-linux vm check`", args.join(" "));
    }
    Ok(())
}

pub(crate) fn check(explicit_checkout: Option<&Path>) -> Result<serde_json::Value> {
    let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let os_fields = parse_os_release(&os_release);
    let distro_id = os_fields
        .get("ID")
        .cloned()
        .unwrap_or_else(|| "unknown".into());
    let pretty_name = os_fields
        .get("PRETTY_NAME")
        .cloned()
        .unwrap_or_else(|| distro_id.clone());
    let apt_available = command_exists("apt-get");
    let supported = supports_apt_host(
        &distro_id,
        os_fields.get("ID_LIKE").map(String::as_str),
        apt_available,
    );

    let checkout = resolve_checkout(explicit_checkout);
    let (checkout_path, checkout_ready, checkout_error) = match checkout {
        Ok(path) => (Some(path.display().to_string()), true, None),
        Err(error) => (None, false, Some(format!("{error:#}"))),
    };
    let vagrant_version = command_output("vagrant", &["--version"]);
    let virsh_version = command_output("virsh", &["--version"]);
    let provider_plugin = command_output("vagrant", &["plugin", "list"])
        .map(|output| output.contains("vagrant-libvirt"))
        .unwrap_or(false);
    let libvirt_uri = command_output("virsh", &["-c", "qemu:///system", "uri"]);
    let virt_host_validation = command_output("virt-host-validate", &["qemu"]);
    let mem_available_kib = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|contents| {
            contents
                .lines()
                .find(|line| line.starts_with("MemAvailable:"))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|value| value.parse::<u64>().ok())
        });
    let cpus = std::thread::available_parallelism()
        .map(|cpus| cpus.get())
        .ok();
    let kvm = std::fs::metadata("/dev/kvm")
        .map(|_| {
            std::fs::OpenOptions::new()
                .read(true)
                .open("/dev/kvm")
                .is_ok()
        })
        .unwrap_or(false);
    let virtiofsd = virtiofsd_available();
    let config = user_vm_data_dir()
        .ok()
        .and_then(|dir| read_settings(&dir).ok())
        .map(|settings| {
            serde_json::json!({
                "checkout": settings.checkout,
                "box": settings.box_name,
                "box_version": settings.box_version,
                "cpus": settings.cpus,
                "memory_mib": settings.memory_mib,
                "sync_mode": settings.sync_mode,
            })
        });
    let ready = preflight_ready(
        supported,
        kvm,
        vagrant_version.is_some(),
        virsh_version.is_some(),
        provider_plugin,
        libvirt_uri.as_deref() == Some("qemu:///system"),
        checkout_ready,
    );

    Ok(serde_json::json!({
        "host": {
            "os_id": distro_id,
            "pretty_name": pretty_name,
            "package_manager": if apt_available { "apt" } else { "unavailable" },
            "supported": supported,
            "supported_host_guidance": if supported { serde_json::Value::Null } else { serde_json::Value::String("Automated setup supports Debian/Ubuntu-compatible apt hosts; install the KVM/libvirt provider manually and rerun this read-only check.".into()) },
        },
        "virtualization": {
            "kvm_device": std::path::Path::new("/dev/kvm").exists(),
            "kvm_accessible": kvm,
            "cpu_count": cpus,
            "mem_available_kib": mem_available_kib,
            "virt_host_validate": virt_host_validation,
        },
        "provider": {
            "vagrant_version": vagrant_version,
            "libvirt_version": virsh_version,
            "system_uri": libvirt_uri,
            "vagrant_libvirt_plugin": provider_plugin,
            "virtiofsd_available": virtiofsd,
        },
        "checkout": {
            "path": checkout_path,
            "ready": checkout_ready,
            "error": checkout_error,
        },
        "configuration": config,
        "ready": ready,
    }))
}

pub(crate) fn lifecycle(
    command: &str,
    overrides: &VmOverrides,
    checkout: Option<&Path>,
) -> Result<()> {
    if command == "post-login" {
        return post_login(checkout);
    }
    let data_dir = user_vm_data_dir()?;
    let settings = read_settings(&data_dir)?;
    match command {
        "start" => {
            check_start_resources(&settings)?;
            run_vagrant(&data_dir, &["up", "--provider", "libvirt"])
        }
        "status" => {
            let output = capture_vagrant(&data_dir, &["status", "--machine-readable"])?;
            let state = output
                .lines()
                .filter_map(|line| {
                    let fields = line.splitn(4, ',').collect::<Vec<_>>();
                    (fields.len() == 4 && fields[2] == "state").then_some(fields[3])
                })
                .next_back()
                .unwrap_or("unknown");
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "state": state,
                    "provider": "libvirt",
                    "box": settings.box_name,
                    "box_version": settings.box_version,
                    "cpus": settings.cpus,
                    "memory_mib": settings.memory_mib,
                    "sync_mode": settings.sync_mode,
                    "checkout": settings.checkout,
                }))?
            );
            Ok(())
        }
        "console" => {
            run_vagrant(&data_dir, &["up", "--provider", "libvirt"])?;
            let id_path = data_dir.join(".vagrant/machines/default/libvirt/id");
            let id = std::fs::read_to_string(&id_path).with_context(|| {
                format!(
                    "Vagrant machine identity missing at {}; run `computer-use-linux vm status`",
                    id_path.display()
                )
            })?;
            if !command_exists("virt-viewer") {
                bail!("virt-viewer is not installed. Install it with `sudo apt install virt-viewer`, then rerun `computer-use-linux vm console`.");
            }
            run_command(
                "virt-viewer",
                &["--connect", "qemu:///system", "--wait", id.trim()],
                None,
            )
        }
        "stop" => run_vagrant(&data_dir, &["halt"]),
        "suspend" => run_vagrant(&data_dir, &["suspend"]),
        "destroy" => {
            println!(
                "Destroying this Vagrant guest deletes its guest-local disk and applications."
            );
            if !confirm_stage("Destroy the guest and its local state? This cannot be undone.")? {
                bail!("VM destroy declined; guest state is unchanged")
            }
            run_vagrant(&data_dir, &["destroy", "--force"])
        }
        "optimize" => optimize_profile(&data_dir, overrides),
        _ => bail!("unsupported VM lifecycle command: {command}"),
    }
}

fn check_start_resources(settings: &VmSettings) -> Result<()> {
    let host_cpu_reserve = 2usize;
    let host_memory_reserve = 4096u64;
    let cpus = host_cpu_count() as usize;
    let mem_available_mib = u64::from(host_memory_available_mib().unwrap_or(0));
    let max_cpus = cpus.saturating_sub(host_cpu_reserve).min(8) as u32;
    let max_memory = mem_available_mib
        .saturating_sub(host_memory_reserve)
        .min(8192) as u32;
    if settings.cpus > max_cpus || settings.memory_mib > max_memory {
        bail!("VM start refused: configured {} vCPU/{} MiB exceeds the safe automatic capacity now ({max_cpus} vCPU/{} MiB after reserving {host_cpu_reserve} CPU cores and {host_memory_reserve} MiB RAM). Reduce with `computer-use-linux vm optimize --cpus N --memory-mib N` or free host resources, then rerun start", settings.cpus, settings.memory_mib, max_memory);
    }
    Ok(())
}

fn capture_vagrant(data_dir: &Path, args: &[&str]) -> Result<String> {
    let settings = read_settings(data_dir)?;
    let output = Command::new("vagrant")
        .args(args)
        .current_dir(data_dir)
        .env("COMPUTER_USE_LINUX_VM_CHECKOUT", &settings.checkout)
        .env("COMPUTER_USE_LINUX_VM_CPUS", settings.cpus.to_string())
        .env(
            "COMPUTER_USE_LINUX_VM_MEMORY_MIB",
            settings.memory_mib.to_string(),
        )
        .env("COMPUTER_USE_LINUX_VM_SYNC", &settings.sync_mode)
        .env("VAGRANT_DEFAULT_PROVIDER", "libvirt")
        .output()
        .with_context(|| format!("could not run `vagrant {}`", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "`vagrant {}` failed: {}; provider remains fixed to libvirt",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn optimize_profile(data_dir: &Path, overrides: &VmOverrides) -> Result<()> {
    let mut settings = read_settings(data_dir)?;
    let (cpus, memory_mib) =
        if let (Some(cpus), Some(memory)) = (overrides.cpus, overrides.memory_mib) {
            (cpus, memory)
        } else {
            let automatic =
                default_resources(host_cpu_count(), host_memory_available_mib().unwrap_or(0))?;
            (
                overrides.cpus.unwrap_or(automatic.0),
                overrides.memory_mib.unwrap_or(automatic.1),
            )
        };
    validate_resources(cpus, memory_mib)?;
    let sync_mode = resolve_sync_mode(
        overrides
            .sync_mode
            .as_deref()
            .unwrap_or(&settings.sync_mode),
        virtiofsd_available(),
        provider_plugin_installed(),
    )?;
    println!(
        "Current VM profile: {} vCPU, {} MiB RAM, {} sync",
        settings.cpus, settings.memory_mib, settings.sync_mode
    );
    println!(
        "Proposed VM-local profile: {cpus} vCPU, {memory_mib} MiB RAM, {sync_mode} sync; virtio devices and SPICE remain enabled."
    );
    println!("No host governor, kernel, swap, or security settings are changed.");
    if !confirm_stage("Apply the VM-local profile? The guest may restart.")? {
        bail!("optimization declined; current profile is unchanged")
    }
    let state = capture_vagrant(data_dir, &["status", "--machine-readable"])?;
    if state
        .lines()
        .any(|line| line.splitn(4, ',').nth(3) == Some("running"))
    {
        run_vagrant(data_dir, &["halt"])?;
    }
    settings.cpus = cpus;
    settings.memory_mib = memory_mib;
    settings.sync_mode = sync_mode;
    std::fs::write(
        data_dir.join("settings.json"),
        serde_json::to_vec_pretty(&settings)?,
    )?;
    println!("Applied VM-local profile. Start the guest with `computer-use-linux vm start`.");
    Ok(())
}

fn post_login(explicit_checkout: Option<&Path>) -> Result<()> {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let desktop = format!(
        "{} {}",
        std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
        std::env::var("DESKTOP_SESSION").unwrap_or_default()
    )
    .to_ascii_lowercase();
    validate_post_login_context(
        Path::new("/etc/computer-use-linux-vm-guest").is_file(),
        &session,
        wayland,
        &desktop,
    )?;
    let current_dir;
    let checkout_path = if let Some(path) = explicit_checkout {
        path
    } else {
        current_dir = std::env::current_dir()
            .context("could not determine the current directory inside the guest")?;
        &current_dir
    };
    let checkout = validate_checkout(checkout_path).context(
        "run `computer-use-linux vm post-login` from the source checkout mounted inside the guest, or pass its guest path with --checkout",
    )?;
    let installer = checkout.join("install.sh");
    println!("Running the existing installer without rebuilding. Complete any portal prompt in this guest session.");
    run_command(
        &installer.to_string_lossy(),
        &["--skip-system-deps", "--skip-rust", "--skip-build"],
        Some(&checkout),
    )
}

fn validate_post_login_context(
    provisioned_guest: bool,
    session_type: &str,
    wayland_display: bool,
    desktop: &str,
) -> Result<()> {
    if !provisioned_guest {
        bail!("post-login setup is restricted to the provisioned computer-use-linux guest; this host is not marked as that guest");
    }
    if session_type != "wayland"
        || !wayland_display
        || !desktop.to_ascii_lowercase().contains("gnome")
    {
        bail!("post-login setup must run inside the guest's logged-in GNOME Wayland session; current session reports XDG_SESSION_TYPE={session_type:?}, WAYLAND_DISPLAY present={wayland_display}, desktop={desktop:?}");
    }
    Ok(())
}

fn supports_apt_host(id: &str, id_like: Option<&str>, apt_available: bool) -> bool {
    if !apt_available {
        return false;
    }
    let id = id.to_ascii_lowercase();
    let family = id_like.unwrap_or_default().to_ascii_lowercase();
    matches!(id.as_str(), "debian" | "ubuntu" | "anduinos")
        && (matches!(id.as_str(), "debian" | "ubuntu")
            || family
                .split_whitespace()
                .any(|name| name == "debian" || name == "ubuntu"))
}

fn preflight_ready(
    supported: bool,
    kvm: bool,
    vagrant: bool,
    libvirt: bool,
    plugin: bool,
    system_uri: bool,
    checkout: bool,
) -> bool {
    supported && kvm && vagrant && libvirt && plugin && system_uri && checkout
}

fn parse_os_release(contents: &str) -> std::collections::HashMap<String, String> {
    contents
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            let value = value.trim().trim_matches('"').trim_matches('\'');
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

fn command_exists(program: &str) -> bool {
    Command::new("sh")
        .args(["-c", "command -v -- \"$1\" >/dev/null 2>&1", "sh", program])
        .status()
        .is_ok_and(|status| status.success())
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        default_resources, execute_setup_stages, is_checkout, preflight_ready, read_settings,
        resolve_sync_mode, stage_assets, supports_apt_host, validate_post_login_context,
        validate_resources, write_settings, SetupStage, VmOverrides, VmSettings,
    };

    #[test]
    fn embedded_assets_stage_outside_the_source_tree() {
        let temp = std::env::temp_dir().join(format!(
            "computer-use-linux-vm-assets-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        stage_assets(&temp).unwrap();
        let vagrantfile = std::fs::read_to_string(temp.join("Vagrantfile")).unwrap();
        let provision = std::fs::read_to_string(temp.join("provision.sh")).unwrap();
        assert!(vagrantfile.contains("bento/ubuntu-26.04"));
        assert!(vagrantfile.contains("qemu:///system"));
        assert!(provision.contains("gnome-shell"));
        assert!(provision
            .contains("/usr/share/gnome-shell/extensions/computer-use-linux@avifenesh.dev"));
        assert!(provision.contains("/etc/dconf/profile/user"));
        assert!(provision.contains("system-db:local"));
        assert!(provision.contains("/etc/dconf/db/local.d/00-computer-use-linux-vm"));
        assert!(provision.contains("enabled-extensions=['computer-use-linux@avifenesh.dev']"));
        assert!(provision.contains("toolkit-accessibility=true"));
        assert!(provision.contains("rm -rf /home/vagrant/.local/share/gnome-shell/extensions/computer-use-linux@avifenesh.dev"));
        assert!(provision.contains("'AutomaticLoginEnable': 'True'"));
        assert!(provision.contains("'AutomaticLogin': 'vagrant'"));
        assert!(provision.contains("/etc/xdg/autostart/computer-use-linux-vm-first-login.desktop"));
        assert!(provision
            .contains("rm -f /home/vagrant/.local/state/computer-use-linux/vm-first-login-done"));
        assert!(provision.contains("doctor still reports blockers; setup will retry"));
        assert!(provision.contains("vm-first-login.log"));
        assert!(provision.contains("Another ydotoold is running with the same socket"));
        assert!(provision.contains("/home/vagrant/.config/systemd/user/ydotool.service"));
        assert!(provision.contains("ln -sfn /dev/null"));
        assert!(provision.contains("systemctl disable --now ydotool.service"));
        assert!(provision.contains("systemctl --user stop ydotool.service ydotoold.service"));
        assert!(provision.contains("gnome-keyring"));
        assert!(provision.contains("unset XDG_SESSION_ID"));
        let _ = std::fs::remove_dir_all(temp);
    }

    #[test]
    fn current_repository_matches_the_fork_checkout_signature() {
        assert!(is_checkout(&std::env::current_dir().unwrap()));
    }

    #[test]
    fn preflight_accepts_a_ready_host_and_rejects_each_missing_gate() {
        assert!(preflight_ready(true, true, true, true, true, true, true));
        for missing in 0..7 {
            let mut gates = [true; 7];
            gates[missing] = false;
            assert!(!preflight_ready(
                gates[0], gates[1], gates[2], gates[3], gates[4], gates[5], gates[6]
            ));
        }
    }

    #[test]
    fn apt_matrix_includes_debian_compatible_derivatives_but_guides_others() {
        assert!(supports_apt_host("anduinos", Some("debian"), true));
        assert!(supports_apt_host("ubuntu", None, true));
        assert!(supports_apt_host("debian", None, true));
        assert!(!supports_apt_host("linuxmint", Some("ubuntu debian"), true));
        assert!(!supports_apt_host("fedora", None, true));
        assert!(!supports_apt_host("debian", None, false));
    }

    #[test]
    fn declining_initial_stage_runs_no_host_changes() {
        let stages = [
            SetupStage {
                name: "plan",
                summary: "plan".into(),
            },
            SetupStage {
                name: "packages",
                summary: "packages".into(),
            },
        ];
        let mut completed = Vec::new();
        let mut applied = Vec::new();
        let result = execute_setup_stages(
            &stages,
            &mut completed,
            |_| Ok(false),
            |stage| {
                applied.push(stage.name);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(completed.is_empty());
        assert!(applied.is_empty());
    }

    #[test]
    fn declining_later_stage_preserves_prior_work_and_stops_future_stages() {
        let stages = [
            SetupStage {
                name: "plan",
                summary: "plan".into(),
            },
            SetupStage {
                name: "packages",
                summary: "packages".into(),
            },
            SetupStage {
                name: "groups",
                summary: "groups".into(),
            },
            SetupStage {
                name: "plugin",
                summary: "plugin".into(),
            },
        ];
        let mut completed = Vec::new();
        let mut applied = Vec::new();
        let mut confirmations = 0;
        let result = execute_setup_stages(
            &stages,
            &mut completed,
            |_| {
                confirmations += 1;
                Ok(confirmations < 3)
            },
            |stage| {
                applied.push(stage.name);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(completed, ["plan", "packages"]);
        assert_eq!(applied, ["plan", "packages"]);
    }

    #[test]
    fn setup_stage_failure_reports_completed_stages_and_stops() {
        let stages = [
            SetupStage {
                name: "packages",
                summary: "packages".into(),
            },
            SetupStage {
                name: "groups",
                summary: "groups".into(),
            },
        ];
        let mut completed = Vec::new();
        let result = execute_setup_stages(
            &stages,
            &mut completed,
            |_| Ok(true),
            |stage| {
                if stage.name == "groups" {
                    anyhow::bail!("mock failure")
                } else {
                    Ok(())
                }
            },
        );
        let error = result.unwrap_err();
        assert!(
            format!("{error:#}").contains("completing [packages]"),
            "unexpected setup error: {error:#}"
        );
        assert_eq!(completed, ["packages"]);
    }

    #[test]
    fn vm_resource_overrides_are_bounded() {
        assert!(validate_resources(1, 2048).is_ok());
        assert!(validate_resources(8, 8192).is_ok());
        assert!(validate_resources(0, 4096).is_err());
        assert!(validate_resources(9, 4096).is_err());
        assert!(validate_resources(2, 2047).is_err());
        assert!(validate_resources(2, 8193).is_err());
    }

    #[test]
    fn sync_selection_reports_virtiofs_or_rsync_without_provider_fallback() {
        assert_eq!(resolve_sync_mode("auto", true, true).unwrap(), "virtiofs");
        assert_eq!(resolve_sync_mode("auto", false, true).unwrap(), "rsync");
        assert_eq!(resolve_sync_mode("rsync", true, true).unwrap(), "rsync");
        assert!(resolve_sync_mode("virtiofs", false, true).is_err());
    }

    #[test]
    fn per_user_settings_keep_resource_overrides_and_explicit_rsync() {
        let dir = std::env::temp_dir().join(format!(
            "computer-use-linux-vm-settings-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let overrides = VmOverrides {
            cpus: Some(4),
            memory_mib: Some(6144),
            sync_mode: Some("rsync".into()),
        };
        write_settings(&dir, Path::new("/tmp/fork-checkout"), &overrides).unwrap();
        let settings = read_settings(&dir).unwrap();
        assert_eq!(settings.cpus, 4);
        assert_eq!(settings.memory_mib, 6144);
        assert_eq!(settings.sync_mode, "rsync");
        assert_eq!(settings.box_name, "bento/ubuntu-26.04");
        assert_eq!(settings.box_version, "202606.01.0");
        let vagrantfile = include_str!("../assets/vm/Vagrantfile");
        assert!(vagrantfile.contains("VAGRANT_DEFAULT_PROVIDER\"] ||= \"libvirt\""));
        assert!(vagrantfile.contains("qemu:///system"));
        assert!(vagrantfile.contains("host-passthrough"));
        assert!(vagrantfile.contains("disk_bus = \"virtio\""));
        assert!(vagrantfile.contains("nic_model_type = \"virtio\""));
        assert!(vagrantfile.contains("graphics_type = \"spice\""));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn automatic_resources_keep_host_reserves_floor_and_caps() {
        assert_eq!(default_resources(24, 16_384).unwrap(), (8, 8192));
        assert_eq!(default_resources(6, 12_288).unwrap(), (4, 8192));
        assert_eq!(default_resources(4, 8192).unwrap(), (2, 4096));
        assert!(default_resources(2, 8192).is_err());
        assert!(default_resources(8, 8191).is_err());
    }

    #[test]
    fn post_login_requires_the_marked_guest_and_active_gnome_wayland() {
        assert!(validate_post_login_context(false, "wayland", true, "gnome").is_err());
        assert!(validate_post_login_context(true, "x11", false, "gnome").is_err());
        assert!(validate_post_login_context(true, "wayland", false, "gnome").is_err());
        assert!(validate_post_login_context(true, "wayland", true, "gnome").is_ok());
    }

    #[test]
    fn machine_readable_box_rows_require_matching_provider_version_and_architecture() {
        let settings = VmSettings {
            checkout: PathBuf::from("/tmp/project"),
            box_name: "bento/ubuntu-26.04".into(),
            box_version: "202606.01.0".into(),
            cpus: 2,
            memory_mib: 4096,
            sync_mode: "rsync".into(),
        };
        let listing = "1,,box-name,bento/ubuntu-26.04\n1,,box-provider,virtualbox\n1,,box-version,202606.01.0\n1,,box-architecture,amd64";
        assert!(!super::box_list_contains(listing, &settings, "amd64"));
        let matching = "1,,box-name,bento/ubuntu-26.04\n1,,box-provider,libvirt\n1,,box-version,202606.01.0\n1,,box-architecture,amd64";
        assert!(super::box_list_contains(matching, &settings, "amd64"));
        assert!(!super::box_list_contains(matching, &settings, "arm64"));
    }
}
