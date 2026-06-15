package head

import "embed"

// siteFS holds the account dashboard served at the head's apex. It is a single
// self-contained page (inlined CSS + JS) styled with the same Cursor Dark
// tokens as the SPA and the gateway login page, so the account home looks like
// the rest of arbos.
//
//go:embed site/index.html
var siteFS embed.FS
