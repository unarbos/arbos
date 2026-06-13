package forest

import "embed"

//go:embed site/index.html site/style.css site/main.js site/favicon.ico site/favicon-32.png site/favicon-96x96.png site/apple-touch-icon.png
var siteFS embed.FS
