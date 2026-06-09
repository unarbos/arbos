#!/usr/bin/env bash
# Install arbos — one command from anywhere.
#
#   curl -fsSL https://raw.githubusercontent.com/unarbos/arbos/main/scripts/install.sh | bash
#
# Or from a clone:
#
#   ./scripts/install.sh
if [ -z "${BASH_VERSION:-}" ]; then
	if command -v bash >/dev/null 2>&1; then
		exec bash -s "$@"
	fi
	echo "arbos: bash is required (dash/sh is not enough). Run: curl ... | bash" >&2
	exit 1
fi
set -euo pipefail

MODULE=github.com/unarbos/arbos/cmd/arbos@latest

if ! command -v go >/dev/null 2>&1; then
	echo "arbos: Go is required. Install from https://go.dev/dl/ then re-run this script." >&2
	exit 1
fi

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

echo ""
echo "Next:"
echo "  export OPENROUTER_API_KEY=sk-or-...   # https://openrouter.ai/keys"
echo "  cd your-project"
echo "  arbos"
