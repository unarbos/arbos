# Arbos

```bash
curl -fsSL https://arbos.life | bash -s -- web
```

## Development

Run the backend and the Vite dev server in two terminals — the SPA proxies
`/api` and `/raw` to the gateway, so the UI hot-reloads against a live agent:

```bash
arbos -web :8420        # or: make run
cd web && npm run dev   # http://localhost:5173, HMR
```

The committed `web/dist` is the UI that ships (`go:embed` bakes it into the
binary). After changing anything under `web/src`, rebuild it before committing —
CI fails if the committed bundle is stale:

```bash
make web                # npm ci && vite build
```

## Releasing

Tag the next version and push it; the release workflow builds and publishes the
binaries. (Use this rather than `gh workflow run`, which needs repo admin.)

```bash
make release                 # bump the patch from the latest tag
make release VERSION=v0.2.0  # explicit version
```

