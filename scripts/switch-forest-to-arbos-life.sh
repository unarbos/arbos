#!/usr/bin/env bash
# Switch the Nebius forest head from constantinople.cloud to arbos.life.
# Run on the box as const after arbos.life NS point at Cloudflare (matias/nancy).
set -euo pipefail

BOX_IP="204.12.163.231"
CF_TOKEN="$(grep SAVED_CF_Token ~/.acme.sh/account.conf | cut -d"'" -f2)"
export CF_Token="${CF_TOKEN}"
export PATH="${HOME}/.acme.sh:${PATH}"

TLS_DIR="${HOME}/.config/arbos-forest/tls-arbos"
STATE="${HOME}/.config/arbos-forest/devices.json"
BIN="${HOME}/forest-head-linux"

echo "Checking public DNS for arbos.life..."
RESOLVED="$(dig +short arbos.life A | head -1 || true)"
if [[ "${RESOLVED}" != "${BOX_IP}" ]]; then
	echo "arbos.life resolves to '${RESOLVED:-<empty>}', expected ${BOX_IP}." >&2
	echo "Update Namecheap nameservers to matias.ns.cloudflare.com and nancy.ns.cloudflare.com first." >&2
	exit 1
fi

echo "Issuing wildcard cert for arbos.life..."
mkdir -p "${TLS_DIR}"
acme.sh --issue --dns dns_cf \
	-d arbos.life -d '*.arbos.life' \
	--keylength ec-256 \
	--cert-file "${TLS_DIR}/cert.pem" \
	--key-file "${TLS_DIR}/key.pem" \
	--fullchain-file "${TLS_DIR}/fullchain.pem" \
	--force

if [[ -f "${HOME}/forest-head-linux.new" ]]; then
	mv "${HOME}/forest-head-linux.new" "${BIN}"
	chmod +x "${BIN}"
fi

echo "Installing systemd unit..."
sudo tee /etc/systemd/system/forest-head.service >/dev/null <<UNIT
[Unit]
Description=arbos forest head (arbos.life)
After=network-online.target

[Service]
User=const
ExecStart=${BIN} --addr :443 --domain arbos.life --tls-cert ${TLS_DIR}/fullchain.pem --tls-key ${TLS_DIR}/key.pem --state ${STATE}
AmbientCapabilities=CAP_NET_BIND_SERVICE
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
UNIT

sudo systemctl daemon-reload
sudo systemctl restart forest-head.service
sudo systemctl status forest-head.service --no-pager

echo ""
echo "Forest head live at https://arbos.life"
echo "Test: curl -fsSL https://arbos.life | head -5"
