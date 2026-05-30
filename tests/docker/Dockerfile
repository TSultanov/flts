# Multi-stage build for the FLTS sync harness (Go engine + Rust, Linux).
#
# Builder compiles `sync-harness`, whose build.rs cross-builds the Syncthing Go
# c-archive (syncthing-core) for this arch and links it. Runtime is a slim image
# with just the binary.

FROM rust:1-trixie AS builder
ARG TARGETARCH
ARG GO_VERSION=1.26.3
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && curl -fsSL "https://go.dev/dl/go${GO_VERSION}.linux-${TARGETARCH}.tar.gz" \
       | tar -C /usr/local -xz
ENV PATH=/usr/local/go/bin:$PATH
ENV GOPATH=/go

WORKDIR /src
COPY . .
RUN cargo build --release -p sync-harness

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /src/target/release/sync-harness /usr/local/bin/sync-harness
ENTRYPOINT ["sync-harness"]
