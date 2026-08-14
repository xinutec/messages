# Multi-stage build: Angular frontend + Rust backend in one image (the backend
# serves the bundle + API). Mirrors the fleet's xinutec/<app>:latest convention.

# --- frontend ---
FROM node:24-alpine AS frontend
WORKDIR /fe
# pnpm-workspace.yaml belongs in this layer, not with the sources: it carries the
# install-script allowlist, and without it neither esbuild nor the ui-harness
# unpacks — the build then fails on dependencies that look installed.
COPY frontend/package.json frontend/pnpm-lock.yaml frontend/pnpm-workspace.yaml ./
# git: the shared layout harness is a git dependency (github:xinutec/ui-harness),
# so the install clones it — node:alpine ships no git.
#
# pnpm is taken unpinned. The host gets its copy from the flake, and pinning a
# second version here would be two numbers held level by hand; the lockfile is
# what has to match, and --frozen-lockfile fails rather than drift.
RUN apk add --no-cache git ca-certificates \
    && npm install -g pnpm \
    && pnpm install --frozen-lockfile
COPY frontend/ .
RUN pnpm exec ng build --configuration production

# --- backend (deps cached in their own layer) ---
FROM rust:1-bookworm AS backend
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && echo '' > src/lib.rs \
    && cargo build --release && rm -rf src
COPY src/ src/
RUN touch src/main.rs src/lib.rs && cargo build --release

# --- runtime ---
FROM debian:bookworm-slim
# openssh-client is the send path: irssi runs on another cluster, so saying
# something as Pippijn means an ssh to amun with a key pinned there to one
# command. Nothing else in the image needs it, and without it the app boots
# fine and refuses every send — which is a much quieter failure than it sounds,
# so it is worth knowing this line is what that would mean.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates openssh-client \
    && rm -rf /var/lib/apt/lists/*
# 65532 is the conventional "nonroot" id, matched by k8s/01-app.yaml.
RUN groupadd --gid 65532 messages \
    && useradd --uid 65532 --gid messages --no-create-home --shell /usr/sbin/nologin messages
WORKDIR /app
COPY --from=backend /app/target/release/messages /usr/local/bin/messages
COPY --from=frontend /fe/dist/messages-web/browser ./public
ENV STATIC_DIR=/app/public \
    BIND_ADDR=0.0.0.0:8080
USER messages
EXPOSE 8080
CMD ["messages"]
