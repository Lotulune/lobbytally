#!/usr/bin/env bash
# Install mpgs-server as a systemd unit (requires root).
# Usage: sudo ./install.sh /path/to/package-root
set -euo pipefail

PKG_ROOT="${1:-.}"
BIN_SRC="${PKG_ROOT}/bin/mpgs-server"
DBTOOL_SRC="${PKG_ROOT}/bin/mpgs-dbtool"
UNIT_SRC="${PKG_ROOT}/linux/mpgs-server.service"
ENV_SRC="${PKG_ROOT}/common/mpgs.env.example"

if [[ "$(id -u)" -ne 0 ]]; then
  echo "run as root" >&2
  exit 1
fi
if [[ ! -x "$BIN_SRC" && ! -f "$BIN_SRC" ]]; then
  echo "missing binary: $BIN_SRC" >&2
  exit 1
fi

id -u mpgs >/dev/null 2>&1 || useradd --system --home /var/lib/mpgs --shell /usr/sbin/nologin mpgs
install -d -o mpgs -g mpgs -m 0750 /var/lib/mpgs /var/log/mpgs /etc/mpgs
install -m 0755 "$BIN_SRC" /usr/local/bin/mpgs-server
if [[ -f "$DBTOOL_SRC" ]]; then
  install -m 0755 "$DBTOOL_SRC" /usr/local/bin/mpgs-dbtool
fi
install -m 0644 "$UNIT_SRC" /etc/systemd/system/mpgs-server.service
if [[ ! -f /etc/mpgs/mpgs.env ]]; then
  install -m 0640 -o root -g mpgs "$ENV_SRC" /etc/mpgs/mpgs.env
  echo "created /etc/mpgs/mpgs.env — set MPGS_DATABASE_PATH and MPGS_ADMIN_TOKEN before start"
fi

systemctl daemon-reload
systemctl enable mpgs-server.service
echo "installed. edit /etc/mpgs/mpgs.env then: systemctl start mpgs-server"
