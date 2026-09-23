# Angular frontend and Rust backend in one image; the backend serves both.

# --- frontend ---
FROM node:24-alpine AS frontend
WORKDIR /fe
# pnpm-workspace.yaml goes in this layer: its install-script allowlist is what
# lets esbuild and the ui-harness unpack.
COPY frontend/package.json frontend/pnpm-lock.yaml frontend/pnpm-workspace.yaml ./
# git, for the ui-harness git dependency. pnpm is unpinned; --frozen-lockfile
# is what must match.
RUN apk add --no-cache git ca-certificates \
    && npm install -g pnpm \
    && pnpm install --frozen-lockfile
COPY frontend/ .
# Stamp the version into the bundle (frontend/scripts/stamp-version.mjs).
# Explicit, since `ng` runs directly and skips the `prebuild` hook. The context
# has no .git, so CI passes GIT_SHA; without it the stamp is `dev`.
ARG GIT_SHA=dev
RUN GIT_SHA="$GIT_SHA" node scripts/stamp-version.mjs
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
# openssh-client is the send path, an ssh to irssi on amun. Without it the app
# runs and refuses every send.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates openssh-client \
    && rm -rf /var/lib/apt/lists/*
# 65532, the conventional "nonroot" id.
RUN groupadd --gid 65532 messages \
    && useradd --uid 65532 --gid messages --no-create-home --shell /usr/sbin/nologin messages
WORKDIR /app
COPY --from=backend /app/target/release/messages /usr/local/bin/messages
# The link-fetch service, the only binary that reaches the open internet.
COPY --from=backend /app/target/release/link-fetch /usr/local/bin/link-fetch
COPY --from=frontend /fe/dist/messages-web/browser ./public
ENV STATIC_DIR=/app/public \
    BIND_ADDR=0.0.0.0:8080
USER messages
EXPOSE 8080
CMD ["messages"]
