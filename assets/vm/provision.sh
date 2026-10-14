#!/usr/bin/env bash
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive
touch /etc/computer-use-linux-vm-guest
if [[ ! -e /var/lib/computer-use-linux-vm-provisioned ]]; then
  apt-get update
  apt-get install -y --no-install-recommends \
    at-spi2-core build-essential ca-certificates curl dbus-x11 dconf-cli gdm3 gnome-keyring \
    gir1.2-atspi-2.0 gnome-session gnome-shell gnome-terminal tilix \
    libatspi2.0-dev libclang-dev libdbus-1-dev libevdev-dev \
    libgbm-dev libinput-dev libssl-dev libudev-dev libwayland-dev \
    libxkbcommon-dev pkg-config spice-vdagent xdg-desktop-portal-gnome

  if ! grep -q '^X-GNOME-Autostart-Phase=Applications$' /etc/xdg/autostart/spice-vdagent.desktop 2>/dev/null; then
    mkdir -p /etc/xdg/autostart
    cat > /etc/xdg/autostart/spice-vdagent.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=SPICE agent
Exec=spice-vdagent
X-GNOME-Autostart-enabled=true
EOF
  fi

fi

# Prepare session defaults before GDM creates the vagrant desktop session.
# Installing the extension system-wide and enabling it through dconf means
# GNOME loads it during the first Shell startup; no logout/restart is needed.
extension_src=/workspace/computer-use-linux/gnome-shell-extension/computer-use-linux@avifenesh.dev
extension_dst=/usr/share/gnome-shell/extensions/computer-use-linux@avifenesh.dev
if [[ ! -f "$extension_src/metadata.json" || ! -f "$extension_src/extension.js" ]]; then
  echo "GNOME extension assets are missing from the source checkout" >&2
  exit 1
fi
install -d -m 0755 "$extension_dst"
install -m 0644 "$extension_src/metadata.json" "$extension_dst/metadata.json"
install -m 0644 "$extension_src/extension.js" "$extension_dst/extension.js"
# A prior installer run may have seeded a per-user extension copy. GNOME gives
# that copy precedence over the system copy, so remove only this stale
# app-owned directory before the fresh Shell session starts.
rm -rf /home/vagrant/.local/share/gnome-shell/extensions/computer-use-linux@avifenesh.dev
install -d -o vagrant -g vagrant -m 0755 /home/vagrant/.local/share/gnome-shell/extensions
# The guest may use a system dconf profile with a database name other than
# "local". Use GNOME's standard local profile and database so our pre-login
# extension and accessibility defaults always participate in the user stack.
install -d -m 0755 /etc/dconf/profile
cat > /etc/dconf/profile/user <<'EOF'
user-db:user
system-db:local
EOF
install -d -m 0755 /etc/dconf/db/local.d
cat > /etc/dconf/db/local.d/00-computer-use-linux-vm <<'EOF'
[org/gnome/desktop/interface]
toolkit-accessibility=true

[org/gnome/desktop/session]
idle-delay=uint32 0

[org/gnome/desktop/screensaver]
lock-enabled=false
lock-delay=uint32 0

[org/gnome/shell]
enabled-extensions=['computer-use-linux@avifenesh.dev']
EOF
dconf update

# This dedicated Vagrant guest has no user data to protect behind a login
# prompt. Start its desktop unattended so the extension is loaded on initial
# Shell startup and first-login setup can finish without console interaction.
python3 - <<'PY'
from pathlib import Path
import re

path = Path('/etc/gdm3/custom.conf')
lines = path.read_text().splitlines() if path.exists() else []
if not any(line.strip() == '[daemon]' for line in lines):
    lines.extend(['[daemon]'])
start = next(i for i, line in enumerate(lines) if line.strip() == '[daemon]')
end = next((i for i in range(start + 1, len(lines)) if lines[i].lstrip().startswith('[')), len(lines))
section = lines[start + 1:end]
options = {'AutomaticLoginEnable': 'True', 'AutomaticLogin': 'vagrant'}
for key, value in options.items():
    pattern = re.compile(r'^\s*#?\s*' + re.escape(key) + r'\s*=')
    matches = [i for i, line in enumerate(section) if pattern.match(line)]
    if matches:
        first = matches[0]
        section[first] = f'{key} = {value}'
        for duplicate in reversed(matches[1:]):
            del section[duplicate]
    else:
        section.append(f'{key} = {value}')
lines[start + 1:end] = section
path.write_text('\n'.join(lines) + '\n')
PY

# Complete the existing user installer automatically after GNOME Shell has
# loaded its system extension set. The one-shot runs inside Wayland and retries
# at the next graphical boot if doctor still reports blockers.
install -d -o vagrant -g vagrant -m 0755 /home/vagrant/.local/bin
install -d -o vagrant -g vagrant -m 0755 /home/vagrant/.local/state/computer-use-linux
cat > /usr/local/bin/computer-use-linux-vm-first-login <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
marker="$HOME/.local/state/computer-use-linux/vm-first-login-done"
[[ -e "$marker" ]] && exit 0
mkdir -p "$(dirname "$marker")"
export XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=GNOME
# A user manager may retain the SSH session id even for its graphical
# autostart children. Let the exported graphical type take precedence.
unset XDG_SESSION_ID
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
export DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-unix:path=$XDG_RUNTIME_DIR/bus}"
export WAYLAND_DISPLAY="${WAYLAND_DISPLAY:-wayland-0}"
export DISPLAY="${DISPLAY:-:0}"

run_installer() {
  /workspace/computer-use-linux/target/release/computer-use-linux vm post-login --checkout /workspace/computer-use-linux
}
doctor_ready() {
  /home/vagrant/.local/bin/computer-use-linux doctor | python3 -c 'import json,sys; d=json.load(sys.stdin); sys.exit(0 if not d["readiness"]["blockers"] else 1)'
}
# Ubuntu's guest image enables its packaged ydotool.service in the user
# manager. It starts ydotoold on the same private socket the installer owns.
# Stop and disable that conflicting guest unit before the idempotent install.
systemctl --user stop ydotoold.service >>"$HOME/.local/state/computer-use-linux/vm-first-login.log" 2>&1 || true
pkill -u "$(id -u)" -x ydotoold 2>/dev/null || true
rm -f "$XDG_RUNTIME_DIR/.ydotool_socket"
if run_installer >"$HOME/.local/state/computer-use-linux/vm-first-login.log" 2>&1; then
  if doctor_ready; then touch "$marker"; exit 0; fi
  echo "Computer Use doctor still reports blockers; setup will retry on the next graphical login." >&2
  exit 1
fi

# Ubuntu's base image may run the distribution ydotoold unit on this same
# socket. Disable that duplicate unit, then retry install.sh so its private
# per-user service owns the socket and the regular installer path succeeds.
if grep -Fq -- 'Another ydotoold is running with the same socket' "$HOME/.local/state/computer-use-linux/vm-first-login.log"; then
  systemctl disable --now ydotool.service >>"$HOME/.local/state/computer-use-linux/vm-first-login.log" 2>&1 || true
  systemctl --user stop ydotool.service ydotoold.service >>"$HOME/.local/state/computer-use-linux/vm-first-login.log" 2>&1 || true
  pkill -u "$(id -u)" -x ydotoold 2>/dev/null || true
  rm -f "$XDG_RUNTIME_DIR/.ydotool_socket"
  if run_installer >>"$HOME/.local/state/computer-use-linux/vm-first-login.log" 2>&1 && doctor_ready; then
    touch "$marker"
    exit 0
  fi
fi

echo "Computer Use first-login setup failed; inspect $HOME/.local/state/computer-use-linux/vm-first-login.log. It will retry on the next graphical login." >&2
exit 1
EOF
chmod 0755 /usr/local/bin/computer-use-linux-vm-first-login
cat > /etc/xdg/autostart/computer-use-linux-vm-first-login.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=Computer Use first-login setup
Exec=/usr/local/bin/computer-use-linux-vm-first-login
X-GNOME-Autostart-enabled=true
X-GNOME-Autostart-Delay=10
EOF
install -d -o vagrant -g vagrant -m 0755 /home/vagrant/.config/systemd/user
ln -sfn /dev/null /home/vagrant/.config/systemd/user/ydotool.service
if [[ -S /run/user/1000/bus ]]; then
  runuser -u vagrant -- env HOME=/home/vagrant XDG_RUNTIME_DIR=/run/user/1000 \
    DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus systemctl --user daemon-reload || true
  runuser -u vagrant -- env HOME=/home/vagrant XDG_RUNTIME_DIR=/run/user/1000 \
    DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus systemctl --user disable --now ydotool.service || true
fi
chown vagrant:vagrant /home/vagrant/.local/bin
# Any provisioning pass can change the guest binary or setup logic. Force the
# next graphical boot to rerun the idempotent session installer and doctor.
rm -f /home/vagrant/.local/state/computer-use-linux/vm-first-login-done

# Keep a screenshot fallback available when GNOME Shell or portal access is
# denied to a particular MCP host. Reinstalling this package is idempotent.
apt-get install -y --no-install-recommends gnome-screenshot ripgrep ydotool

# Make the uinput fallback available to the unprivileged graphical guest user.
# The rule is scoped to this disposable VM's sole desktop account and keeps the
# ydotoold socket private; it does not expose input injection to other users.
mkdir -p /etc/modules-load.d /etc/udev/rules.d
printf '%s\n' 'uinput' > /etc/modules-load.d/computer-use-linux.conf
printf '%s\n' 'KERNEL=="uinput", OWNER="vagrant", GROUP="vagrant", MODE="0600", OPTIONS+="static_node=uinput"' \
  > /etc/udev/rules.d/60-computer-use-linux-uinput.rules
modprobe uinput
udevadm trigger --action=add --name-match=uinput
udevadm settle
chown vagrant:vagrant /dev/uinput
chmod 0600 /dev/uinput

if [[ ! -d /workspace/computer-use-linux ]]; then
  echo "Expected source checkout at /workspace/computer-use-linux" >&2
  exit 1
fi

# This phase runs on every `vagrant up --provision`. Cargo reuses its build
# cache, while rebuilding when the shared checkout has changed; it also repairs
# an interrupted first provision that installed packages but missed the build.
runuser -u vagrant -- env HOME=/home/vagrant PATH=/home/vagrant/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/bin bash -c '
if ! command -v rustup >/dev/null 2>&1; then
  curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
export PATH="$HOME/.cargo/bin:$PATH"
rustup toolchain install stable
rustup default stable
cd /workspace/computer-use-linux
cargo build --locked --release
'
systemctl set-default graphical.target
systemctl enable gdm3

if [[ ! -e /var/lib/computer-use-linux-vm-provisioned ]]; then
  touch /var/lib/computer-use-linux-vm-provisioned
fi
