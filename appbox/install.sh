#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALL_BIN="${HOME}/.local/bin"
mkdir -p "$INSTALL_BIN"

cat > "${INSTALL_BIN}/appbox" <<EOF
#!/usr/bin/env bash
PYTHONPATH="${SCRIPT_DIR}\${PYTHONPATH:+:\$PYTHONPATH}" exec python3 -m appbox "\$@"
EOF
chmod +x "${INSTALL_BIN}/appbox"

echo "==> Installed CLI to ${INSTALL_BIN}/appbox"

"${INSTALL_BIN}/appbox" setup-nautilus
echo "==> AppBox installation complete!"
