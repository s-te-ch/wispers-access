# syntax=docker/dockerfile:1
#
# Wispers Access connector image: the `waserver` binary plus a process
# supervisor. One container hosts many shares — the entrypoint reconciles the
# desired shares from config and runs one `waserver serve` per share under
# supervisord, which owns signal forwarding, restart, and zombie reaping.
#
# Build context is the repo root (so the Rust workspace is available):
#     docker build -f connector/Dockerfile -t wispers-connector .
# or just use connector/compose.yaml, which sets the context for you.

# --- builder --------------------------------------------------------------
FROM rust:1-bookworm AS builder
WORKDIR /src
# Native build deps for wispers-connect's stack:
#   cmake + clang/libclang  -> boring-sys (BoringSSL build + bindgen FFI)
#   protobuf-compiler       -> prost/tonic codegen from .proto files
# (the C/C++ toolchain itself is already in the rust image.)
RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake clang libclang-dev protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*
# Only the workspace crates are needed to build waserver. (target/ is excluded
# by .dockerignore.) No dependency-caching layer yet — see README for the
# cargo-chef optimisation when iteration speed matters.
COPY Cargo.toml Cargo.lock ./
COPY waserver ./waserver
COPY waclient ./waclient
RUN cargo build --release -p waserver

# --- runtime --------------------------------------------------------------
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
         supervisor ca-certificates bash \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/waserver /usr/local/bin/waserver
COPY connector/entrypoint.sh    /usr/local/bin/entrypoint.sh
COPY connector/healthcheck.sh   /usr/local/bin/healthcheck.sh
COPY connector/supervisord.conf /etc/supervisor/supervisord.conf
RUN chmod +x /usr/local/bin/entrypoint.sh /usr/local/bin/healthcheck.sh \
    && mkdir -p /etc/supervisor/conf.d

# All persistent state (per-share identity: keys + registration), plus IPC
# sockets and logs, live under $HOME. Mount a volume at /data to persist the
# identity across restarts/redeploys — without it, every boot creates a new
# connectivity group.
ENV HOME=/data
VOLUME ["/data"]

# Healthy once every configured share reports "serving". The start-period
# covers the "connecting" window while shares dial the hub.
HEALTHCHECK --interval=30s --timeout=5s --start-period=45s --retries=3 \
  CMD ["/usr/local/bin/healthcheck.sh"]

# entrypoint.sh reconciles shares, then `exec`s supervisord as PID 1.
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
