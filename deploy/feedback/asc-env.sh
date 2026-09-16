#!/bin/sh
# Exports the App Store Connect credentials for asc-feedback.py from the
# 1Password Arbos vault, so no credential file has to live on the host.
# Needs `op` signed in (a service-account token in OP_SERVICE_ACCOUNT_TOKEN).
# Usage:  . deploy/feedback/asc-env.sh && python3 deploy/feedback/asc-feedback.py
# Nothing here prints a value.
ITEM="${ASC_OP_ITEM:-phsrnmu3qfpqx6lbumafjo3uom}"   # "Appe Cert Apple Developer"
KEY_ID="${ASC_KEY_ID:-ZV82D3ZWRT}"                   # "Arbos IOS", Admin
export ASC_KEY_ID="$KEY_ID"
ASC_KEY_P8="$(op document get "$ITEM" --vault Arbos --file-name "AuthKey_${KEY_ID}.p8" 2>/dev/null || op read "op://Arbos/$ITEM/AuthKey_${KEY_ID}.p8")"
export ASC_KEY_P8
# The issuer id is a uuid inside the item's note; nothing else in the note is read out.
ASC_ISSUER_ID="$(op read "op://Arbos/$ITEM/notesPlain" | grep -oE '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}' | head -1)"
export ASC_ISSUER_ID
[ -n "$ASC_KEY_P8" ] && [ -n "$ASC_ISSUER_ID" ] || { echo "asc-env: could not read the key or the issuer from the vault" >&2; return 1 2>/dev/null || exit 1; }
