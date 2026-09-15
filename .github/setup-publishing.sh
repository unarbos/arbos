#!/usr/bin/env bash
#
# Set this repository up to publish updates. Run once, from a checkout.
#
#     .github/setup-publishing.sh
#
# It does four things, and prints no secret value at any point:
#
#   1. Makes the Arbos update signing key if the tree has no public half yet,
#      writes the public half into crates/arbos-update/update-key.pub for you
#      to commit, and pipes the private half straight into the repository
#      secret ARBOS_UPDATE_SIGNING_KEY. This is the key every Arbos app checks
#      a payload against, and it is the one thing the dev channel cannot run
#      without.
#
#   2. Reads Jacob's Apple code-signing certificate out of the 1Password
#      "Arbos" vault and puts it in the repository secrets, so CI can sign the
#      macOS payload. The certificate never touches the working tree.
#
#   3. Puts the Apple ID, team, and app-specific password in as well, which is
#      what notarization needs.
#
#   4. Says what is set and what is still missing.
#
# Needs: the GitHub CLI signed in with admin on this repository (`gh auth
# login`), and the 1Password CLI signed in to the Arbos vault (`op signin`, or
# OP_SERVICE_ACCOUNT_TOKEN in the environment).
#
# ── which certificate ────────────────────────────────────────────────────────
# Apple's notary service accepts exactly one kind of certificate, "Developer ID
# Application". Anything else — an "Apple Development" certificate, say — can
# sign a build but can never be notarized.
#
# So the two are kept in two secrets and mean two different things:
#
#   DEVELOPER_ID_CERT_P12    a Developer ID Application certificate. CI signs,
#   P12_PASSWORD             notarizes and staples. Gatekeeper is happy with a
#                            copy downloaded in a browser.
#
#   APPLE_SIGNING_CERT_P12   any other Apple code-signing certificate. CI signs
#   APPLE_SIGNING_CERT_PASSWORD  and does not notarize.
#
# The vault item today holds an *Apple Development* certificate, so this script
# puts it in the second pair. Both paths publish; the in-app updater installs
# either, because what it checks is Arbos's own signature over the payload.
# When a Developer ID Application certificate exists, export it from Keychain
# Access and re-run this script: it goes into the first pair and notarization
# turns itself on with no other change anywhere.

set -euo pipefail

VAULT="Arbos"
# "Appe Cert Apple Developer" — the certificates and the two passwords.
CERT_ITEM="phsrnmu3qfpqx6lbumafjo3uom"
# The item carries two exports. `IDApple.p12` is the Developer ID Application
# certificate, the only kind Apple will notarize, so it is the one to use.
# `AppCert.p12` is an Apple Development certificate: it can sign and can never
# be notarized, and is the fallback only for a vault that has not been given
# the other one yet.
CERT_FILES="IDApple.p12 AppCert.p12"
# The two CONCEALED fields on that item share a label, so they are named by id.
# The first opens both .p12 exports; the second is the app-specific password
# notarytool submits with.
CERT_P12_PASSWORD_FIELD="kt3mumvelyuuk7vhzy5yi5sfwi"
CERT_APP_PASSWORD_FIELD="lubqkq5h7qlwttdbdg3w3i476e"
# "Apple id" — the address notarytool submits as.
APPLE_ID_ITEM="irqui2abi45albacpga432mana"
APPLE_TEAM_ID="25SCF3Q2AK"

KEY_FILE="crates/arbos-update/update-key.pub"

cd "$(dirname "$0")/.."

have() { command -v "$1" >/dev/null 2>&1; }
say() { printf '%s\n' "$*"; }

have gh || { say "gh is not installed: https://cli.github.com"; exit 1; }
have op || { say "op is not installed: https://developer.1password.com/docs/cli"; exit 1; }
gh auth status >/dev/null 2>&1 || { say "gh is not signed in — run: gh auth login"; exit 1; }

repo="$(gh repo view --json nameWithOwner -q .nameWithOwner)"
say "repository: $repo"
say ""

# Everything secret lands here and nowhere else, and goes away on the way out
# however this script ends.
work="$(mktemp -d)"
cleanup() { rm -rf "$work"; }
trap cleanup EXIT INT TERM
umask 077

# ── 1. the update signing key ────────────────────────────────────────────────
if grep -qE '^ed25519 ' "$KEY_FILE" 2>/dev/null; then
  say "update key: $KEY_FILE already holds a public key, leaving it alone."
  say "            (to roll it, delete that line and re-run — every app built"
  say "             against the old key will stop accepting updates.)"
else
  say "update key: making one."
  cargo run -q -p arbos-update --bin arbos-updatectl -- keygen --public "$KEY_FILE" \
    | gh secret set ARBOS_UPDATE_SIGNING_KEY --repo "$repo"
  say "            ARBOS_UPDATE_SIGNING_KEY set; commit $KEY_FILE."
fi
say ""

# ── 2. the Apple certificate ─────────────────────────────────────────────────
say "certificate: reading it out of the $VAULT vault."
op item get "$CERT_ITEM" --vault "$VAULT" --format json --reveal \
  | python3 -c "
import json, sys
item = json.load(sys.stdin)
want = {'$CERT_P12_PASSWORD_FIELD': 'p12-password', '$CERT_APP_PASSWORD_FIELD': 'app-password'}
for field in item['fields']:
    name = want.get(field['id'])
    if name:
        open('$work/' + name, 'w').write(field['value'])
"
test -s "$work/p12-password" || { say "no .p12 password on that vault item"; exit 1; }
test -s "$work/app-password" || { say "no app-specific password on that vault item"; exit 1; }

# Read the leaf's common name out of a .p12.
#
# Three ways, because there is no one way that works everywhere. The exports
# are old-format PKCS#12: OpenSSL 3 needs `-legacy` for that, LibreSSL — which
# is what `/usr/bin/openssl` is on a Mac — has no such flag and reads them
# without one. And `security` reads them natively, which is both the last
# resort and the same thing CI does.
cert_common_name() {
  local p12="$1" subject=""
  subject="$(openssl pkcs12 -in "$p12" -passin "file:$work/p12-password" \
    -nokeys -clcerts -legacy 2>/dev/null | openssl x509 -noout -subject 2>/dev/null || true)"
  if [ -z "$subject" ]; then
    subject="$(openssl pkcs12 -in "$p12" -passin "file:$work/p12-password" \
      -nokeys -clcerts 2>/dev/null | openssl x509 -noout -subject 2>/dev/null || true)"
  fi
  if [ -n "$subject" ]; then
    printf '%s' "$subject" | sed 's/.*CN *= *\([^,]*\).*/\1/'
    return 0
  fi
  if have security; then
    local keychain="$work/read.keychain-db" pass
    pass="$(openssl rand -hex 16)"
    security create-keychain -p "$pass" "$keychain" >/dev/null 2>&1 || return 1
    security unlock-keychain -p "$pass" "$keychain" >/dev/null 2>&1 || true
    security import "$p12" -P "$(cat "$work/p12-password")" -A -t cert -f pkcs12 \
      -k "$keychain" >/dev/null 2>&1 || true
    security find-identity -v -p codesigning "$keychain" \
      | sed -n 's/.*"\(.*\)".*/\1/p' | head -1
    security delete-keychain "$keychain" >/dev/null 2>&1 || true
    return 0
  fi
  return 1
}

# Take the first export on the item that opens, preferring the Developer ID.
common_name=""
chosen=""
for name in $CERT_FILES; do
  op read "op://$VAULT/$CERT_ITEM/$name" --out-file "$work/cert.p12" >/dev/null 2>&1 || continue
  common_name="$(cert_common_name "$work/cert.p12" || true)"
  if [ -n "$common_name" ]; then
    chosen="$name"
    break
  fi
  say "            $name would not open with the password on the item; skipping."
done
if [ -z "$chosen" ]; then
  say "            no usable .p12 on that vault item."
  exit 1
fi
say "            $chosen — $common_name"

base64 -w0 < "$work/cert.p12" > "$work/cert.b64" 2>/dev/null \
  || base64 < "$work/cert.p12" | tr -d '\n' > "$work/cert.b64"

case "$common_name" in
  "Developer ID Application:"*)
    gh secret set DEVELOPER_ID_CERT_P12 --repo "$repo" < "$work/cert.b64"
    gh secret set P12_PASSWORD --repo "$repo" < "$work/p12-password"
    say "            DEVELOPER_ID_CERT_P12 and P12_PASSWORD set."
    say "            builds will be signed, notarized and stapled."
    notarizes=yes
    ;;
  *)
    gh secret set APPLE_SIGNING_CERT_P12 --repo "$repo" < "$work/cert.b64"
    gh secret set APPLE_SIGNING_CERT_PASSWORD --repo "$repo" < "$work/p12-password"
    say "            APPLE_SIGNING_CERT_P12 and APPLE_SIGNING_CERT_PASSWORD set."
    say "            builds will be signed and NOT notarized: Apple's notary"
    say "            service takes only a \"Developer ID Application\""
    say "            certificate, and this is not one."
    notarizes=no
    ;;
esac
# The Developer ID export carries the leaf and the key and no intermediate, so
# `codesign` needs Apple's "Developer ID Certification Authority" in the
# keychain to build a chain to the root. A Mac has it; a fresh runner keychain
# may not, and the workflow fetches it there. Nothing to do here.
say ""

# ── 3. notarization credentials ──────────────────────────────────────────────
# Two ways to prove to Apple's notary service who is asking, and the API key
# is the better one: it is made for this, it can be revoked without touching
# the Apple ID, and no account password is involved. Both are set where the
# material exists, and the build prefers the key.
say "notarize:   setting the credentials."

# The App Store Connect API key. The key id is in the file's own name —
# `AuthKey_XANFG7S7YS.p8` is key `XANFG7S7YS` — which is how Apple ships it.
key_file="$(op item get "$CERT_ITEM" --vault "$VAULT" --format json \
  | python3 -c "
import json, sys
print(next((f['name'] for f in json.load(sys.stdin).get('files', [])
            if f['name'].endswith('.p8')), ''))
")"
if [ -n "$key_file" ]; then
  op read "op://$VAULT/$CERT_ITEM/$key_file" --out-file "$work/notary.p8" >/dev/null
  key_id="$(printf '%s' "$key_file" | sed -n 's/^AuthKey_\([A-Z0-9]*\)\.p8$/\1/p')"
  # The issuer id is a UUID. A field of its own is the tidy place for it; the
  # note is where it is today, so it is read from there when there is exactly
  # one UUID in it and nothing to mistake it for.
  issuer_source="a labelled field"
  op item get "$CERT_ITEM" --vault "$VAULT" --format json --reveal \
    | python3 -c "
import json, re, sys, uuid
item = json.load(sys.stdin)
def is_uuid(v):
    try:
        uuid.UUID(v or ''); return True
    except ValueError:
        return False
labelled = [f['value'] for f in item['fields']
            if f.get('label') != 'notesPlain' and is_uuid(f.get('value'))]
if labelled:
    open('$work/issuer', 'w').write(labelled[0]); sys.exit(0)
note = next((f.get('value') or '' for f in item['fields']
             if f.get('label') == 'notesPlain'), '')
found = set(re.findall(
    r'[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}', note))
if len(found) == 1:
    open('$work/issuer', 'w').write(found.pop())
    open('$work/issuer-from-note', 'w').write('yes')
"
  [ -f "$work/issuer-from-note" ] && issuer_source="the item's note"
  if [ -s "$work/issuer" ] && [ -n "$key_id" ]; then
    gh secret set NOTARY_KEY_P8 --repo "$repo" < "$work/notary.p8"
    printf '%s' "$key_id" | gh secret set NOTARY_KEY_ID --repo "$repo"
    gh secret set NOTARY_ISSUER_ID --repo "$repo" < "$work/issuer"
    say "            $key_file → NOTARY_KEY_P8, NOTARY_KEY_ID (key $key_id),"
    say "            NOTARY_ISSUER_ID (issuer id read from $issuer_source)."
  else
    say "            $key_file is there but its issuer id is not — App Store"
    say "            Connect › Users and Access › Integrations shows it. Put it"
    say "            on the vault item as its own field and re-run."
  fi
else
  say "            no App Store Connect .p8 on the vault item."
fi

# The Apple ID fallback.
op item get "$APPLE_ID_ITEM" --vault "$VAULT" --format json --reveal \
  | python3 -c "
import json, sys
note = json.load(sys.stdin)['fields'][0]['value']
address = next((line.strip() for line in note.splitlines() if '@' in line), '')
open('$work/apple-id', 'w').write(address)
"
test -s "$work/apple-id" || { say "no email address on the Apple id item"; exit 1; }
gh secret set APPLE_ID --repo "$repo" < "$work/apple-id"
printf '%s' "$APPLE_TEAM_ID" | gh secret set APPLE_TEAM_ID --repo "$repo"
gh secret set APPLE_APP_PASSWORD --repo "$repo" < "$work/app-password"
say "            APPLE_ID, APPLE_TEAM_ID and APPLE_APP_PASSWORD set as the"
say "            fallback."
say ""

# ── 4. where that leaves us ──────────────────────────────────────────────────
say "done."
say ""
say "  merge to main      → .github/workflows/dev-channel.yml publishes a"
say "                       signed build and updates the dev feed"
say "  push a v* tag      → .github/workflows/release.yml publishes the"
say "                       tagged release and updates the stable feed"
say ""
if [ "$notarizes" = "no" ]; then
  say "still missing: a Developer ID Application certificate. Without it the"
  say "macOS payload is signed but not notarized. The in-app updater installs"
  say "it regardless — Arbos checks its own signature over the payload — but a"
  say "copy downloaded in a browser is stopped by Gatekeeper."
  say ""
  say "To make one: developer.apple.com → Certificates, Identifiers & Profiles"
  say "→ Certificates → + → Developer ID Application. Export it from Keychain"
  say "Access as a .p12, put it on the vault item, and re-run this script."
fi
