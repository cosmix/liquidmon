#!/usr/bin/env bash
# Installs liquidctl's udev rules so unprivileged users can access supported
# devices. Run with sudo. Pulls the canonical rules file from upstream main.
#
# Usage:  sudo ./scripts/install-liquidctl-udev.sh

set -euo pipefail

RULES_URL="https://raw.githubusercontent.com/liquidctl/liquidctl/main/extra/linux/71-liquidctl.rules"
DEST="/etc/udev/rules.d/71-liquidctl.rules"

if [[ $EUID -ne 0 ]]; then
    echo "error: must be run as root (use sudo)" >&2
    exit 1
fi

if ! command -v curl >/dev/null 2>&1; then
    echo "error: curl is required but not installed" >&2
    exit 1
fi

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT

echo "Downloading $RULES_URL"
curl --fail --silent --show-error --location "$RULES_URL" -o "$tmp"

# Sanity check: file should be non-trivial and contain expected marker.
if [[ ! -s "$tmp" ]] || ! grep -q 'liquidctl' "$tmp"; then
    echo "error: downloaded rules file looks invalid" >&2
    exit 1
fi

install -m 0644 -o root -g root "$tmp" "$DEST"
echo "Installed $DEST"

udevadm control --reload
udevadm trigger
echo "Reloaded and triggered udev rules."

cat <<'EOF'

Done. Existing /dev/hidraw* nodes keep their old permissions until the device
is rebound or the system reboots. If `liquidctl status` still requires sudo,
either unplug-and-replug the AIO's USB connection (header on the motherboard,
typically the internal USB 2.0) or reboot.

Verify with:
    liquidctl --json status
EOF
