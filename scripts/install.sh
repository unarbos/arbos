#!/usr/bin/env bash
# Install arbos — one command from anywhere.
#
#   curl -fsSL https://arbos.life | bash -s -- arbos       # install + TUI
#   curl -fsSL https://arbos.life | bash -s -- arbos web  # install + web UI
#
# Or from a clone:
#
#   ./scripts/install.sh           # install + TUI
#   ./scripts/install.sh arbos web # install + web UI
if [ -z "${BASH_VERSION:-}" ]; then
	if command -v bash >/dev/null 2>&1; then
		exec bash -s "$@"
	fi
	echo "arbos: bash is required (dash/sh is not enough). Run: curl ... | bash" >&2
	exit 1
fi
set -euo pipefail

# @main fetched directly from GitHub (proxy bypassed): the module proxy
# caches @latest for up to ~30 minutes, which would hand fresh installs a
# stale default forest head right after a release.
MODULE=github.com/unarbos/arbos/cmd/arbos@main
export GOPROXY=direct GOSUMDB=off GOFLAGS=-buildvcs=false
GO_VERSION=1.26.4
GO_ROOT="${HOME}/.local/go"
# Where the release workflow publishes prebuilt binaries (versionless asset
# names, so "latest" is a stable URL).
RELEASE_BASE="${ARBOS_RELEASE_BASE:-https://github.com/unarbos/arbos/releases/latest/download}"

detect_platform() {
	OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
	ARCH="$(uname -m)"
	case "${ARCH}" in
	x86_64 | amd64) ARCH=amd64 ;;
	aarch64 | arm64) ARCH=arm64 ;;
	*) return 1 ;;
	esac
	case "${OS}" in
	linux | darwin) ;;
	*) return 1 ;;
	esac
}

sha256_of() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | awk '{print $1}'
	else
		shasum -a 256 "$1" | awk '{print $1}'
	fi
}

# install_prebuilt downloads this platform's binary from the latest GitHub
# release into BIN_DIR — no Go toolchain, no compile, works on a 512 MB box.
# Returns nonzero (and installs nothing) when there is no matching release
# asset or verification fails; the caller falls back to building from source.
install_prebuilt() {
	detect_platform || return 1
	ASSET="arbos_${OS}_${ARCH}.tar.gz"
	TMP="$(mktemp -d)"
	trap 'rm -rf "${TMP}"' RETURN
	curl -fsSL -o "${TMP}/checksums.txt" "${RELEASE_BASE}/checksums.txt" 2>/dev/null || return 1
	WANT="$(awk -v a="${ASSET}" '$2 == a {print $1}' "${TMP}/checksums.txt")"
	[[ -n "${WANT}" ]] || return 1
	curl -fsSL -o "${TMP}/${ASSET}" "${RELEASE_BASE}/${ASSET}" || return 1
	GOT="$(sha256_of "${TMP}/${ASSET}")"
	if [[ "${GOT}" != "${WANT}" ]]; then
		echo "arbos: ${ASSET} checksum mismatch (got ${GOT}, want ${WANT}) — falling back to source build" >&2
		return 1
	fi
	tar -C "${TMP}" -xzf "${TMP}/${ASSET}" arbos || return 1
	"${TMP}/arbos" --version >/dev/null 2>&1 || return 1
	mkdir -p "${BIN_DIR}"
	# Stage + rename so a concurrently running arbos sees an atomic swap.
	cp "${TMP}/arbos" "${BIN_DIR}/arbos.new"
	chmod +x "${BIN_DIR}/arbos.new"
	mv "${BIN_DIR}/arbos.new" "${BIN_DIR}/arbos"
}

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

# BIN_DIR matches where `go install` puts binaries (and where the launcher
# script looks) even when Go itself never gets installed: GOPATH's default
# is $HOME/go. Resolved without running `go` — invoking it can trigger a
# toolchain download, which the prebuilt path exists to avoid.
BIN_DIR="${GOPATH:-${HOME}/go}/bin"

# A successful install is silent: the only thing printed on the happy path is
# the binary's own output (the login URL). Progress and "installed" notices
# would otherwise bury the one line worth reading. Errors still speak up.
if ! install_prebuilt; then
	# No release asset for this platform (or no release yet): build @main
	# from source, bootstrapping a Go toolchain if needed.
	ensure_go
	BIN_DIR="$(go env GOPATH)/bin"
	go install "${MODULE}"
fi

# Persist BIN_DIR before mutating our own PATH, otherwise the check below
# always matches and the rc files never get updated.
case ":${PATH}:" in
*":${BIN_DIR}:"*) ;;
*)
	LINE="export PATH=\"${BIN_DIR}:\$PATH\""
	for rc in "${HOME}/.bashrc" "${HOME}/.zshrc"; do
		if [[ -f "${rc}" ]] && ! grep -qE "go env GOPATH\)/bin|\\\$HOME/go/bin|${BIN_DIR}" "${rc}" 2>/dev/null; then
			echo "" >>"${rc}"
			echo "# arbos" >>"${rc}"
			echo "${LINE}" >>"${rc}"
			echo "Added ${BIN_DIR} to PATH via ${rc}"
		fi
	done
	;;
esac

export PATH="${BIN_DIR}:${PATH}"

# Only speak up when the binary is not reachable on PATH — otherwise stay quiet
# so the login URL is the sole output (warnings go to stderr).
if ! command -v arbos >/dev/null 2>&1; then
	echo "arbos installed to ${BIN_DIR}/arbos — add it to PATH:" >&2
	echo "  export PATH=\"${BIN_DIR}:\$PATH\"" >&2
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

	# macOS: chromedp finds Chrome by its .app bundle, not via PATH, so detect
	# the app and install with Homebrew (resolved directly — a piped,
	# non-login shell may not have brew on PATH). NONINTERACTIVE keeps the cask
	# install from prompting and hanging the pipe.
	if [[ "$(uname -s)" == "Darwin" ]]; then
		for app in "/Applications/Google Chrome.app" "${HOME}/Applications/Google Chrome.app" \
			"/Applications/Chromium.app" "${HOME}/Applications/Chromium.app"; do
			[[ -d "${app}" ]] && return 0
		done
		BREW=""
		if command -v brew >/dev/null 2>&1; then BREW="brew"
		elif [[ -x /opt/homebrew/bin/brew ]]; then BREW="/opt/homebrew/bin/brew"
		elif [[ -x /usr/local/bin/brew ]]; then BREW="/usr/local/bin/brew"; fi
		if [[ -n "${BREW}" ]]; then
			echo "Installing Google Chrome for the agent's browser tool (best-effort)..."
			if NONINTERACTIVE=1 "${BREW}" install --cask google-chrome >/dev/null 2>&1; then
				echo "Installed Google Chrome."
				return 0
			fi
		fi
		echo "Note: no Chrome/Chromium found — install one to enable the agent's browser tool:"
		echo "  brew install --cask google-chrome"
		return 0
	fi

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

install_launcher() {
	launcher="${HOME}/.local/bin/arbos"
	mkdir -p "$(dirname "${launcher}")"
	if curl -fsSL https://arbos.life/arbos -o "${launcher}" 2>/dev/null ||
		curl -fsSL https://raw.githubusercontent.com/unarbos/arbos/main/scripts/arbos -o "${launcher}" 2>/dev/null; then
		chmod +x "${launcher}"
	fi
}
install_launcher

args=("$@")
if [[ ${#args[@]} -gt 0 && ${args[0]} == arbos ]]; then
	args=("${args[@]:1}")
fi

# Default to the web door: a bare install (no args) should bring up the web
# server, not drop into the interactive agent. Use the explicit `web`
# subcommand rather than `arbos .` — the latter only maps to the web door in
# newer builds, while `web` works on every release the installer might fetch.
if [[ ${#args[@]} -eq 0 ]]; then
	args=(web)
fi

exec env PATH="${BIN_DIR}:${PATH}" arbos ${args[@]+"${args[@]}"}
