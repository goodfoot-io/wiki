# Wiki devcontainer — builds on the shared local base image.
# The base (devcontainer-base:node24.16-trixie) is built on demand by the
# initializeCommand in devcontainer.json (see ../../devcontainer-base).
FROM devcontainer-base:node24.16-trixie

ARG TZ
ENV TZ="$TZ"

# Repo-specific system packages on top of the base: the headless-browser stack
# for VS Code extension tests, plus Rust profiling tools (this repo has a Rust
# CLI in ./packages).
USER root
RUN apt-get update && export DEBIAN_FRONTEND=noninteractive && apt-get install -y \
  chromium \
  dbus-x11 \
  xvfb \
  mold \
  linux-perf \
  && rm -rf /var/lib/apt/lists/*
USER node

# Yarn workspace-local caches + a larger Node heap.
ENV YARN_CACHE_FOLDER=/workspace/.yarn/cache
ENV YARN_GLOBAL_FOLDER=/workspace/.yarn/global
ENV NODE_OPTIONS=--max-old-space-size=4096

# Repo dev CLIs (installed to the base's system npm prefix, so they survive the
# /home/node bind mount).
RUN npm install -g @anthropic-ai/claude-code @openai/codex && \
  npm install -g tsx dotenv-cli @ast-grep/cli jsdoczoom
