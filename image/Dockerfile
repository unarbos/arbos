# syntax=docker/dockerfile:1.7
# The arbos-agent image (sandbox-image.md §3). Build for linux/amd64 only.
# Pin the base by digest in CI once the tag is chosen (node:24.21.0-bookworm-slim@sha256:...).
FROM node:24.21.0-bookworm-slim

ARG PI_VERSION=1.2.0
ARG PRIME_AGENT_VERSION=0.10.0
ARG CLAUDE_CODE_VERSION=2.1.292
ARG AGENT_BROWSER_VERSION=v0.39.1
ARG CHROME_VERSION=155.0.8059.79
ARG UV_VERSION=0.13.1
ARG RIPGREP_VERSION=15.2.0
ARG GH_VERSION=2.102.0
ARG DOPPLER_VERSION=3.77.0
# Set to 0 to skip the optional Claude Code layer (~250 MB).
ARG WITH_CLAUDE_CODE=1
# Set to 0 to skip Chrome + agent-browser (local smoke builds).
ARG WITH_BROWSER=1

ENV DEBIAN_FRONTEND=noninteractive LANG=C.UTF-8 TZ=UTC

# --- OS packages: build tools, git, Chrome's shared libraries, a fake display, fonts, ffmpeg (F10 video/audio)
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl wget gnupg sudo git openssh-client \
      build-essential pkg-config libssl-dev \
      jq fd-find unzip zip xz-utils procps less ffmpeg \
      xvfb fonts-liberation fonts-noto-color-emoji \
      libnss3 libatk1.0-0 libatk-bridge2.0-0 libcups2 libdrm2 libxkbcommon0 \
      libxcomposite1 libxdamage1 libxfixes3 libxrandr2 libgbm1 libasound2 \
      libpango-1.0-0 libcairo2 libx11-xcb1 libxshmfence1 libnss3-tools \
 && ln -s /usr/bin/fdfind /usr/local/bin/fd \
 && rm -rf /var/lib/apt/lists/*
# A system-level git identity so an agent's first `git commit` in a scratch repo does not fail
# with "Author identity unknown" (#1359). System config has the lowest precedence: a repo or a
# user config the agent (or the owner) sets wins over it.
RUN git config --system user.name "Arbos agent" \
 && git config --system user.email "agent@arbos.life" \
 && git config --system init.defaultBranch main

# --- ripgrep, gh, doppler (pinned release artifacts)
RUN curl -fsSLo /tmp/rg.deb https://github.com/BurntSushi/ripgrep/releases/download/${RIPGREP_VERSION}/ripgrep_${RIPGREP_VERSION}-1_amd64.deb \
 && dpkg -i /tmp/rg.deb && rm /tmp/rg.deb \
 && curl -fsSL https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_linux_amd64.tar.gz \
    | tar -xz -C /usr/local --strip-components=1 gh_${GH_VERSION}_linux_amd64/bin/gh \
 && curl -fsSL https://github.com/DopplerHQ/cli/releases/download/${DOPPLER_VERSION}/doppler_${DOPPLER_VERSION}_linux_amd64.tar.gz \
    | tar -xz -C /usr/local/bin doppler

# --- uv (system-wide) and Python 3.11 for Prime Agent's kernel
ENV UV_PYTHON_INSTALL_DIR=/opt/python
RUN curl -LsSf https://astral.sh/uv/${UV_VERSION}/install.sh | env UV_INSTALL_DIR=/usr/local/bin sh \
 && uv python install 3.11 && chmod -R a+rX /opt/python

# --- The same Python as `python3`, `python` and `pip` on PATH, so an agent's "pip install <sdk or
# cli>" works out of the box: the PEP 668 marker uv ships is dropped (it would refuse every pip
# install), pip falls back to a --user install when site-packages is read-only, and agentd puts
# each workspace's ~/.local/bin on PATH (isolation::env).
RUN PY="$(uv python find 3.11)" \
 && find /opt/python -name EXTERNALLY-MANAGED -delete \
 && ln -s "$PY" /usr/local/bin/python3 && ln -s "$PY" /usr/local/bin/python \
 && python3 -m ensurepip --upgrade \
 && ln -s "$(dirname "$PY")/pip3" /usr/local/bin/pip3 && ln -s /usr/local/bin/pip3 /usr/local/bin/pip \
 && chmod -R a+rX /opt/python

# --- Main agent: pi (needs the Node 24 in the base image) + the Arbos extension
RUN npm install -g --ignore-scripts @earendil-works/pi-coding-agent@${PI_VERSION} \
 && npm cache clean --force

# --- Subagent harness 1: Prime Agent + its Python 3.11 IPython kernel venv, baked now, not at first use
ENV PRIME_AGENT_KERNEL_VENV=/opt/prime-venv PRIME_AGENT_TELEMETRY=0 DO_NOT_TRACK=1
# The release tarball is flat (binary, prime-agent-runtime/, skills/, ...); the kernel runtime is bundled, not on PyPI.
RUN mkdir -p /opt/prime-agent-${PRIME_AGENT_VERSION} \
 && curl -fsSL https://github.com/PrimeIntellect-ai/prime-agent/releases/download/v${PRIME_AGENT_VERSION}/prime-agent-${PRIME_AGENT_VERSION}-linux-x64.tar.gz \
    | tar -xz -C /opt/prime-agent-${PRIME_AGENT_VERSION} \
 && ln -s /opt/prime-agent-${PRIME_AGENT_VERSION}/prime-agent /usr/local/bin/prime-agent \
 && uv venv /opt/prime-venv --python 3.11 --seed \
 && uv pip install --python /opt/prime-venv/bin/python /opt/prime-agent-${PRIME_AGENT_VERSION}/prime-agent-runtime \
      ipykernel dill httpx requests pyyaml tomli python-dotenv pandas numpy scipy beautifulsoup4 lxml \
      python-pptx python-docx openpyxl Pillow reportlab \
 # Prime's Python skills (edit, websearch, compact, goal, refine, attach-image, agent-message,
 # agent-observe, rlm-heartbeat) are packages under the tarball's skills/; a kernel venv without
 # them starts with "Warning: Python skills unavailable … and will be disabled" and Prime runs with
 # no edit or websearch tool (found on production boxes up to image 0.1.49).
 && for d in /opt/prime-agent-${PRIME_AGENT_VERSION}/skills/*/; do \
      if [ -f "$d/pyproject.toml" ]; then uv pip install -q --python /opt/prime-venv/bin/python "$d"; fi; \
    done \
 && HOME=/tmp/pa-build PRIME_AGENT_KERNEL_VENV=/opt/prime-venv PRIME_AGENT_KERNEL_PYTHON=/opt/prime-venv/bin/python \
      prime-agent --prime-agent-bootstrap \
 # Build-time check: Prime must start without the skills warning (stderr empty after a short RPC run).
 && mkdir -p /tmp/pa-check/sessions \
 && ( (echo '{"type":"ping"}'; sleep 8) | HOME=/tmp/pa-check PRIME_AGENT_CODING_AGENT_DIR=/tmp/pa-check/cfg \
      PRIME_AGENT_SESSION_DIR=/tmp/pa-check/sessions PRIME_AGENT_KERNEL_VENV=/opt/prime-venv \
      PRIME_AGENT_KERNEL_PYTHON=/opt/prime-venv/bin/python timeout 30 prime-agent --mode rpc \
      --session-dir /tmp/pa-check/sessions >/dev/null 2>/tmp/pa-check/err.txt || true ) \
 && if grep -q "skills unavailable" /tmp/pa-check/err.txt; then cat /tmp/pa-check/err.txt; exit 1; fi \
 && rm -rf /tmp/pa-check \
 && chmod -R a+rX /opt/prime-venv /opt/prime-agent-${PRIME_AGENT_VERSION}

# --- Subagent harness 2: Claude Code, native binary (no Node), exact version, updates disabled
RUN if [ "$WITH_CLAUDE_CODE" = "1" ]; then \
      curl -fsSL https://claude.ai/install.sh | HOME=/tmp/cc-build bash -s ${CLAUDE_CODE_VERSION} \
   && install -m 755 /tmp/cc-build/.local/share/claude/versions/${CLAUDE_CODE_VERSION} /usr/local/bin/claude \
   && rm -rf /tmp/cc-build; \
    fi


# --- Browser: Chrome for Testing + agent-browser
RUN if [ "$WITH_BROWSER" = "1" ]; then \
      curl -fsSLo /tmp/chrome.zip https://storage.googleapis.com/chrome-for-testing-public/${CHROME_VERSION}/linux64/chrome-linux64.zip \
   && unzip -q /tmp/chrome.zip -d /opt && mv /opt/chrome-linux64 /opt/chrome && rm /tmp/chrome.zip \
   && curl -fsSLo /usr/local/bin/agent-browser https://github.com/vercel-labs/agent-browser/releases/download/${AGENT_BROWSER_VERSION}/agent-browser-linux-x64 \
   && chmod +x /usr/local/bin/agent-browser; \
    fi

# --- Arbos files last, so a code change rebuilds only these thin layers (heavy downloads stay cached)
COPY image/pi-seed/ /etc/arbos/pi-seed/
# The extension is copied by a RUN keyed on this build's AGENTD_SHA, from a bind mount of the build
# context: a `COPY` of these files (glob or directory) kept coming back from the buildx cache with the
# 2026-10-22 03:32 copy on images 0.1.121–0.1.123 while index.ts had changed on main four times (the
# agentd binary in the same images was fresh). The sha changes every build, so this thin layer is
# always rebuilt from the checked-out files; the RUN also records which commit the seed came from.
ARG AGENTD_SHA=unknown
RUN --mount=type=bind,source=harness/arbos-pi-ext,target=/tmp/arbos-pi-ext \
    mkdir -p /etc/arbos/pi-seed/extensions/arbos-pi-ext \
 && cp -a /tmp/arbos-pi-ext/. /etc/arbos/pi-seed/extensions/arbos-pi-ext/ \
 && rm -rf /etc/arbos/pi-seed/extensions/arbos-pi-ext/node_modules \
 && echo "$AGENTD_SHA" > /etc/arbos/pi-seed/extensions/arbos-pi-ext/.source-sha
# typebox resolves from pi's own node_modules; link it next to the extension so jiti finds it.
RUN mkdir -p /etc/arbos/pi-seed/extensions/arbos-pi-ext/node_modules \
 && TB="$(dirname "$(find "$(npm root -g)/@earendil-works" -path '*/node_modules/typebox/package.json' | head -1)")" \
 && ln -s "$TB" /etc/arbos/pi-seed/extensions/arbos-pi-ext/node_modules/typebox \
 && ln -s "$(npm root -g)/@earendil-works" /etc/arbos/pi-seed/extensions/arbos-pi-ext/node_modules/@earendil-works
COPY image/claude-settings.json /etc/arbos/claude-seed/settings.json
COPY image/claude-mcp.json      /etc/arbos/claude-mcp.json
# --- D64 hooks: deny-list + audit shell wrapper (Prime bash()), Claude Code hook scripts, Prime skills
COPY image/arbos-shell          /usr/local/lib/arbos/arbos-shell
COPY image/hooks/               /usr/local/lib/arbos/hooks/
COPY image/prime-skills/        /etc/arbos/prime-seed/skills/
RUN chmod 755 /usr/local/lib/arbos/arbos-shell /usr/local/lib/arbos/hooks/claude/*.sh
# --- D83 real values: every login shell (profile.d) and interactive shell (bash.bashrc) sources
# the workspace's secrets.env; `bash -c` gets it through BASH_ENV from agentd.
COPY image/secrets-env.sh       /usr/local/lib/arbos/secrets-env.sh
RUN chmod 644 /usr/local/lib/arbos/secrets-env.sh \
 && ln -s /usr/local/lib/arbos/secrets-env.sh /etc/profile.d/arbos-secrets.sh \
 && printf '\n. /usr/local/lib/arbos/secrets-env.sh\n' >> /etc/bash.bashrc
# --- Our binary (built earlier in the same CI job: cargo build --release -p arbos-agentd → dist/)
COPY dist/arbos-agentd /usr/local/bin/arbos-agentd
COPY image/AGENTS.md   /etc/arbos/AGENTS.md

# --- User and folders (the node image already owns uid 1000; reuse it for daytona)
RUN userdel -r node \
 && useradd -m -u 1000 -s /bin/bash daytona \
 && echo "daytona ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/91-daytona \
 && groupadd arbos \
 && mkdir -p /work /run/arbos /opt/arbos/shared /home/daytona/.arbos && chown -R daytona:daytona /work /opt/arbos /home/daytona/.arbos \
 && chown root:arbos /opt/arbos/shared && chmod 755 /opt/arbos/shared \
 && chmod 1777 /run/arbos

# The commit and image version agentd reports in its box hello (relay admin `agentd.sha`).
ARG AGENTD_SHA=unknown
ARG IMAGE_VERSION=unknown

# --- Self-reference (docs/agent-self-reference-repo.md): a read-only shallow checkout of the public
# unarbos/arbos tree at this very commit, so an agent asked how Arbos works (encryption, privacy,
# architecture) reads the real code and docs instead of guessing. Root-owned, every write bit off;
# agentd (root) refreshes it to main's tip at boot (selfref.rs). Public repo: no token involved.
# `safe.directory` lets the workspace users run `git log`/`git grep` on a root-owned tree.
ARG ARBOS_SRC_REPO=https://github.com/unarbos/arbos.git
RUN mkdir -p /opt/arbos/src && cd /opt/arbos/src \
 && git init -q && git remote add origin "$ARBOS_SRC_REPO" \
 && ref="$AGENTD_SHA" && case "$ref" in unknown|"") ref=main ;; esac \
 && GIT_TERMINAL_PROMPT=0 git fetch -q --depth 1 origin "$ref" \
 && git checkout -q -B main FETCH_HEAD \
 && git config --system --add safe.directory /opt/arbos/src \
 && chown -R root:root /opt/arbos/src && chmod -R a-w /opt/arbos/src
ENV ARBOS_AGENTD_SHA=$AGENTD_SHA \
    ARBOS_IMAGE_VERSION=$IMAGE_VERSION

ENV HOME=/home/daytona \
    ARBOS_STATE_DIR=/work/.arbos \
    ARBOS_PORT=7788 \
    PRIME_AGENT_CODING_AGENT_DIR=/work/.arbos/prime \
    PRIME_AGENT_SESSION_DIR=/work/.arbos/prime/sessions \
    PRIME_AGENT_BASH_SHELL=/usr/local/lib/arbos/arbos-shell \
    PRIME_AGENT_KERNEL_PYTHON=/opt/prime-venv/bin/python \
    PI_CODING_AGENT_DIR=/work/.arbos/pi \
    PI_TELEMETRY=0 PI_SKIP_VERSION_CHECK=1 \
    CLAUDE_CONFIG_DIR=/work/.arbos/claude \
    DISABLE_AUTOUPDATER=1 DISABLE_UPDATES=1 DISABLE_TELEMETRY=1 DISABLE_ERROR_REPORTING=1 \
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 DISABLE_FEEDBACK_COMMAND=1 \
    AGENT_BROWSER_EXECUTABLE_PATH=/opt/chrome/chrome \
    AGENT_BROWSER_PROFILE=/work/.arbos/browser \
    AGENT_BROWSER_ARGS="--no-sandbox,--disable-dev-shm-usage" \
    AGENT_BROWSER_STREAM_PORT=9223 \
    AGENT_BROWSER_SCREENSHOT_DIR=/work/.arbos/shots \
    ARBOS_MEDIA_VENV=/opt/prime-venv \
    AI_AGENT_SANDBOX=daytona

USER daytona
# Register the project MCP server once, in the image; the token is read from the env at run time
RUN HOME=/home/daytona PRIME_AGENT_CODING_AGENT_DIR=/etc/arbos/prime-seed \
    prime-agent mcp add arbos --url http://127.0.0.1:7789/mcp --bearer-token-env-var ARBOS_MCP_TOKEN || true
WORKDIR /work
EXPOSE 7788
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s \
  CMD curl -fsS http://127.0.0.1:7788/healthz || exit 1
ENTRYPOINT ["/usr/local/bin/arbos-agentd", "boot"]
