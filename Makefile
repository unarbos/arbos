GO ?= go

.PHONY: build install web test vet lint run tidy generate generate-check release e2e-live e2e-fake

# Rebuild the embedded web UI. web/dist is committed (go:embed bakes it into the
# binary, and `go install @main` has no Node), so run this before committing any
# web/src change — CI's web-dist job fails if the committed bundle is stale.
web:
	cd web && npm ci && npm run build

build:
	$(GO) build ./...

# install ships the embedded UI, so rebuild it first — the installed binary is
# only as fresh as web/dist.
install: web
	$(GO) install ./cmd/arbos
	@echo "Installed to $$($(GO) env GOPATH)/bin/arbos"
	@echo "Run: export OPENROUTER_API_KEY=sk-or-... && arbos"

generate:
	$(GO) generate ./...

# generate-check regenerates and fails if anything changed — the ADR-0004
# guarantee that committed tool schemas match the arg structs.
generate-check: generate
	git diff --exit-code

test:
	$(GO) test ./...

vet:
	$(GO) vet ./...

lint:
	golangci-lint run

run:
	$(GO) run ./cmd/arbos

# Cut a release: tag the next patch (or `make release VERSION=v0.2.0`) and push
# it. The release workflow builds and publishes the binaries on the tag — a
# tag push needs only write access, unlike `gh workflow run` (repo admin).
release:
	./scripts/release.sh $(VERSION)

# Broad end-to-end proof: static gate + fake zero-setup + live OpenRouter via doppler.
e2e-live:
	./scripts/e2e-live.sh

e2e-fake:
	./scripts/e2e-live.sh --fake-only

capability-audit:
	./scripts/capability-audit.sh

capability-audit-fake:
	./scripts/capability-audit.sh --fake-only

tidy:
	$(GO) mod tidy
