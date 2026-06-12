#!/usr/bin/env bash
# Install arbos — one command from anywhere.
#
#   curl -fsSL https://raw.githubusercontent.com/unarbos/arbos/main/scripts/install.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/unarbos/arbos/main/scripts/install.sh | bash -s -- web
#
# Or from a clone:
#
#   ./scripts/install.sh
#   ./scripts/install.sh web
if [ -z "${BASH_VERSION:-}" ]; then
	if command -v bash >/dev/null 2>&1; then
		exec bash -s "$@"
	fi
	echo "arbos: bash is required (dash/sh is not enough). Run: curl ... | bash" >&2
	exit 1
fi
set -euo pipefail

MODULE=github.com/unarbos/arbos/cmd/arbos@latest
GO_VERSION=1.26.4
GO_ROOT="${HOME}/.local/go"

ensure_go() {
	if command -v go >/dev/null 2>&1; then
		return 0
	fi
	if [[ -x "${GO_ROOT}/bin/go" ]]; then
		export GOROOT="${GO_ROOT}"
		export PATH="${GO_ROOT}/bin:${PATH}"
		return 0
	fi

	OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
	ARCH="$(uname -m)"
	case "${ARCH}" in
	x86_64 | amd64) ARCH=amd64 ;;
	aarch64 | arm64) ARCH=arm64 ;;
	*)
		echo "arbos: unsupported architecture ${ARCH} — install Go from https://go.dev/dl/ then re-run." >&2
		exit 1
		;;
	esac
	case "${OS}" in
	linux | darwin) ;;
	*)
		echo "arbos: unsupported OS ${OS} — install Go from https://go.dev/dl/ then re-run." >&2
		exit 1
		;;
	esac

	TAR="go${GO_VERSION}.${OS}-${ARCH}.tar.gz"
	URL="https://go.dev/dl/${TAR}"
	echo "arbos: Go not found — downloading ${GO_VERSION} for ${OS}/${ARCH}..."
	TMP="$(mktemp -d)"
	trap 'rm -rf "${TMP}"' RETURN
	if ! curl -fsSL -o "${TMP}/${TAR}" "${URL}"; then
		echo "arbos: failed to download Go from ${URL}" >&2
		exit 1
	fi
	mkdir -p "$(dirname "${GO_ROOT}")"
	rm -rf "${GO_ROOT}"
	tar -C "$(dirname "${GO_ROOT}")" -xzf "${TMP}/${TAR}"

	export GOROOT="${GO_ROOT}"
	export PATH="${GO_ROOT}/bin:${PATH}"

	GO_LINE='export PATH="$HOME/.local/go/bin:$PATH"'
	for rc in "${HOME}/.bashrc" "${HOME}/.zshrc"; do
		if [[ -f "${rc}" ]] && ! grep -qF '.local/go/bin' "${rc}" 2>/dev/null; then
			echo "" >>"${rc}"
			echo "# arbos (Go toolchain)" >>"${rc}"
			echo "${GO_LINE}" >>"${rc}"
			echo "Added Go to PATH via ${rc}"
		fi
	done

	if ! command -v go >/dev/null 2>&1; then
		echo "arbos: Go bootstrap failed — install from https://go.dev/dl/ then re-run." >&2
		exit 1
	fi
	echo "Installed Go ${GO_VERSION} to ${GO_ROOT}"
}

ensure_go

echo "Installing arbos..."
go install "${MODULE}"

BIN_DIR="$(go env GOPATH)/bin"
export PATH="${BIN_DIR}:${PATH}"

case ":${PATH}:" in
*":${BIN_DIR}:"*) ;;
*)
	LINE="export PATH=\"\$(go env GOPATH)/bin:\$PATH\""
	for rc in "${HOME}/.bashrc" "${HOME}/.zshrc"; do
		if [[ -f "${rc}" ]] && ! grep -qF 'go env GOPATH)/bin' "${rc}" 2>/dev/null; then
			echo "" >>"${rc}"
			echo "# arbos" >>"${rc}"
			echo "${LINE}" >>"${rc}"
			echo "Added ${BIN_DIR} to PATH via ${rc}"
		fi
	done
	;;
esac

if command -v arbos >/dev/null 2>&1; then
	echo ""
	echo "Installed: $(command -v arbos)"
else
	echo ""
	echo "Installed to ${BIN_DIR}/arbos — add it to PATH:"
	echo "  export PATH=\"${BIN_DIR}:\$PATH\""
fi

# A real Chromium powers the agent's browser tool (live Browser panels,
# screenshots, logged-in sites). Best-effort and non-fatal: arbos runs fine
# without one — the browser tool just errors with an install hint until a
# browser exists. sudo runs with -n so a piped install never hangs on a
# password prompt.
ensure_browser() {
	for bin in google-chrome google-chrome-stable chromium chromium-browser headless-shell; do
		command -v "${bin}" >/dev/null 2>&1 && return 0
	done
	if [[ "$(uname -s)" != "Linux" ]]; then
		echo "Note: no Chrome/Chromium found — install one to enable the agent's browser tool."
		return 0
	fi
	SUDO=""
	if [[ ${EUID} -ne 0 ]]; then
		if command -v sudo >/dev/null 2>&1; then
			SUDO="sudo -n"
		else
			echo "Note: no Chrome/Chromium found and no sudo — install one to enable the browser tool."
			return 0
		fi
	fi
	echo "Installing Chromium for the agent's browser tool (best-effort)..."
	if command -v apt-get >/dev/null 2>&1; then
		# Google's deb is the most reliable on Ubuntu servers (24.04's apt
		# "chromium-browser" is a snap shim that needs systemd). amd64 only —
		# Google ships no linux/arm64 Chrome.
		if [[ "$(dpkg --print-architecture 2>/dev/null)" == "amd64" ]]; then
			TMP="$(mktemp -d)"
			if curl -fsSL -o "${TMP}/chrome.deb" https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb &&
				${SUDO} apt-get update -qq >/dev/null 2>&1 &&
				${SUDO} apt-get install -y -qq "${TMP}/chrome.deb" >/dev/null 2>&1; then
				rm -rf "${TMP}"
				echo "Installed Google Chrome."
				return 0
			fi
			rm -rf "${TMP}"
		fi
		# Debian (and derivatives with a real package) carry plain "chromium".
		if ${SUDO} apt-get install -y -qq chromium >/dev/null 2>&1; then
			echo "Installed Chromium (apt)."
			return 0
		fi
	fi
	if command -v snap >/dev/null 2>&1 && ${SUDO} snap install chromium >/dev/null 2>&1; then
		echo "Installed Chromium (snap)."
		return 0
	fi
	echo "Note: could not install a browser automatically. To enable the browser tool:"
	echo "  Ubuntu/Debian: sudo apt-get install -y chromium  (or: sudo snap install chromium)"
	echo "  Other:         install Google Chrome or Chromium and ensure it is on PATH"
}
ensure_browser

if [[ $# -gt 0 ]]; then
	exec env PATH="${BIN_DIR}:${PATH}" arbos "$@"
fi

echo ""
echo "Next:"
echo "  export OPENROUTER_API_KEY=sk-or-...   # https://openrouter.ai/keys"
echo "  arbos web      # serve the UI + get a public URL (arbos web --local to stay offline)"
echo ""
echo "Or in a terminal session:"
echo "  cd your-project && arbos"
echo ""
echo "Update later: arbos upgrade (or just tell the agent to update itself)"
