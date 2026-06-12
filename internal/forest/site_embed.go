package forest

import "embed"

//go:embed site/index.html site/style.css site/main.js
var siteFS embed.FS
