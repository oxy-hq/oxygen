# ---------------------------------------------------------------------------------
# NOTE: This Dockerfile is NOT necessary for the development process right now.
# It is reserved for potential usage in the future.
# ---------------------------------------------------------------------------------

# Base image for Rust and Cargo Chef
FROM lukemathwalker/cargo-chef:latest-rust-1.98.1-bookworm AS chef
WORKDIR /app

# Stage 1: Dependency planner
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2: Build the web application
FROM node:26-slim AS web-builder
WORKDIR /app

COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY web-app/package.json ./web-app/
# sdk/* are pnpm-workspace members, so their manifests must exist for
# `pnpm install` to resolve the workspace lockfile — web-app does not depend
# on them, so the sdk itself is never built here.
COPY sdk/typescript/package.json ./sdk/typescript/
COPY sdk/vite-plugin/package.json ./sdk/vite-plugin/
COPY sdk/create-oxy-app/package.json ./sdk/create-oxy-app/
# Node 25 dropped bundled Corepack, so node:26-slim has no `corepack` binary —
# install it from npm. Keeps the sha512-pinned `packageManager` field authoritative.
RUN npm install -g corepack@latest && \
    corepack enable && corepack prepare --activate && pnpm install

COPY web-app/ ./web-app/
ARG VITE_SENTRY_DSN
ENV VITE_SENTRY_DSN=$VITE_SENTRY_DSN
RUN pnpm -C web-app build

# Stage 3: Build the Rust application
FROM chef AS rust-builder

RUN apt-get update && \
    apt-get install -y protobuf-compiler ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
COPY --from=web-builder /app/web-app/dist /app/crates/app/dist
RUN cargo build --release

# Stage 4: Runtime image
FROM debian:bookworm-slim AS runtime
WORKDIR /app

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    ca-certificates \
    tini \
    git \
    chromium \
    fonts-liberation \
    fonts-noto-color-emoji \
    fonts-noto-cjk && \
    rm -rf /var/lib/apt/lists/*

# Headless browser for server-side ECharts rendering (rust-headless-chrome)
# Point to the actual Chromium binary, not the wrapper script
ENV CHROME=/usr/lib/chromium/chromium

COPY --from=rust-builder /app/target/release/oxy /usr/local/bin

# Directory for persistent app data inside the container
ENV OXY_STATE_DIR=/var/lib/oxy/data
RUN mkdir -p ${OXY_STATE_DIR} && chown -R root:root /var/lib/oxy
VOLUME ["${OXY_STATE_DIR}"]

# Set tini as the entrypoint
ENTRYPOINT ["/usr/bin/tini", "--"]

# Default command
EXPOSE 3000 3001
CMD ["oxy", "serve", "--port", "3000", "--internal-host", "0.0.0.0"]