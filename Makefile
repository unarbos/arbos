GO ?= go

.PHONY: build install test vet lint run tidy generate generate-check e2e-live e2e-fake

build:
	$(GO) build ./...

install:
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
