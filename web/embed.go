// Package web embeds the built browser UI so the single arbos binary serves
// it with no extra files on disk: `arbos -web :8420` just works. Rebuild with
// `npm run build` in this directory before `go build` to refresh the bundle.
package web

import "embed"

//go:embed all:dist
var Dist embed.FS
