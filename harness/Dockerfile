# arbos-harness: the kernel, headless, for benchmark containers.
#
#   docker build -f harness/Dockerfile -t arbos-harness \
#     --build-arg ARBOS_GIT_SHA=$(git rev-parse --short=12 HEAD) .
#
# The build context excludes .git, so the kernel cannot learn its own commit;
# pass it as ARBOS_GIT_SHA and `arbos-kernel --version` will say it. Without
# the argument the binary says "unknown", and the harness's kernel_sha check
# cannot pass — a labelled binary must prove its label (QA, 2026-09-17).
#
# Standalone, against a repository checkout:
#   docker run --rm -e OPENROUTER_API_KEY -e ARBOS_MODEL=<model> \
#     -v "$PWD/repo:/testbed" -v "$PWD/out:/logs/artifacts/arbos" \
#     arbos-harness "Fix the failing test in ..."
#
# As a verifiers harness the image is only the source of the static kernel:
# `arbos_harness` copies /usr/local/bin/arbos-kernel out of it and uploads
# it into each task's own container (a SWE-bench instance image), so the
# agent runs where the tests run.
#
# The kernel is linked against musl (static-pie), so the same binary runs in
# any x86_64 Linux image whatever its glibc.

FROM rustlang/rust:nightly-bookworm AS build
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends musl-tools \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# rust-toolchain.toml picks the nightly; the target must join that one, so
# it is added after the file is in place (rustup reads it from the cwd).
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
RUN rustup show active-toolchain && rustup target add x86_64-unknown-linux-musl
COPY crates crates
COPY vendor vendor
# `[patch.crates-io]` in the workspace points at this path; the desktop
# crate itself is not built.
COPY desktop/vendor/bezel-markdown desktop/vendor/bezel-markdown
ARG ARBOS_GIT_SHA=
ENV ARBOS_GIT_SHA=$ARBOS_GIT_SHA
RUN cargo build --release -p arbos-kernel --target x86_64-unknown-linux-musl \
    && strip target/x86_64-unknown-linux-musl/release/arbos-kernel

FROM debian:bookworm-slim
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends \
        git ca-certificates procps \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/x86_64-unknown-linux-musl/release/arbos-kernel /usr/local/bin/arbos-kernel
COPY harness/arbos_harness/arbos-swe-run /usr/local/bin/arbos-swe-run
RUN chmod +x /usr/local/bin/arbos-kernel /usr/local/bin/arbos-swe-run \
    && mkdir -p /logs/artifacts/arbos /testbed
ENV ARBOS_MODE=auto \
    ARBOS_OUT=/logs/artifacts/arbos \
    XDG_CONFIG_HOME=/tmp/arbos-home/config \
    HOME=/root
WORKDIR /testbed
ENTRYPOINT ["/usr/local/bin/arbos-swe-run"]
