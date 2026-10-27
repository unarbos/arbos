#!/usr/bin/env bash
# Build the :443 Caddy with the caddy-l4 module so TURN/TLS (turn.arbos.life, SNI routed to LiveKit's
# 5349 listener) can share port 443 with the sites, and with caddy-dns/cloudflare so the arbos.life
# vhost gets its certificate by DNS-01 (agent sites, #2060: the apex is proxied by Cloudflare, which
# the TLS-ALPN challenge cannot pass). Run on the server as the www user:
#   scripts/deploy/livekit/build-caddy.sh [caddy-version]   → installs $WWW_DIR/bin/caddy (old binary kept as caddy.prev)
# Needs Go (go version) on the box; xcaddy is installed into ~/go/bin on first use.
set -euo pipefail
VER="${1:-v2.11.4}"
WWW_DIR="${WWW_DIR:-$HOME/arbos-www}"
export PATH="$PATH:$HOME/go/bin"
command -v go >/dev/null || { echo "install Go first (https://go.dev/dl)" >&2; exit 1; }
command -v xcaddy >/dev/null || go install github.com/caddyserver/xcaddy/cmd/xcaddy@latest
tmp=$(mktemp -d)
xcaddy build "$VER" --with github.com/mholt/caddy-l4 --with github.com/caddy-dns/cloudflare --output "$tmp/caddy"
"$tmp/caddy" list-modules | grep -q '^layer4$' || { echo "built caddy lacks layer4" >&2; exit 1; }
"$tmp/caddy" list-modules | grep -q '^dns.providers.cloudflare$' || { echo "built caddy lacks dns.providers.cloudflare" >&2; exit 1; }
install -d "$WWW_DIR/bin"
[[ -x "$WWW_DIR/bin/caddy" ]] && cp -a "$WWW_DIR/bin/caddy" "$WWW_DIR/bin/caddy.prev"
install -m 0755 "$tmp/caddy" "$WWW_DIR/bin/caddy"
rm -rf "$tmp"
echo "installed $("$WWW_DIR/bin/caddy" version) with layer4 at $WWW_DIR/bin/caddy; restart arbos-www.service to use it"
