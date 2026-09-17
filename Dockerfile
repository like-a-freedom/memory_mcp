# syntax=docker/dockerfile:1.7

FROM rust:1.97.1-slim-trixie AS builder

WORKDIR /src

# RocksDB and ONNX Runtime are compiled/downloaded by Cargo during the build.
RUN apt-get update \
    && apt-get install --no-install-recommends --yes \
        build-essential \
        ca-certificates \
        cmake \
        git \
        libclang-dev \
        libssl-dev \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY . .

RUN --mount=type=cache,id=memory-mcp-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=memory-mcp-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=memory-mcp-target-trixie,target=/src/target \
    set -eux; \
    cargo build --locked --release -p memory_mcp --bin memory_mcp_http --features streamable-http,control-plane; \
    mkdir -p /out/runtime; \
    install -Dm755 target/release/memory_mcp_http /out/memory_mcp_http; \
    find target/release -maxdepth 1 -type f \( -name '*.so' -o -name '*.so.*' \) -exec cp -v '{}' /out/runtime/ \;

# The runtime is deliberately shell-free and runs as the unprivileged distroless
# user. Keep this stage free of the Rust toolchain, Cargo registry, and sources.
FROM gcr.io/distroless/cc-debian13:nonroot AS runtime

WORKDIR /app

COPY --from=builder /out/memory_mcp_http /usr/local/bin/memory_mcp_http
COPY --from=builder /out/runtime/ /usr/local/lib/
# Registry migrations currently resolve from the compile-time
# `CARGO_MANIFEST_DIR`; keep those SQL assets at that path in the runtime
# image without shipping the source tree or toolchain.
COPY --from=builder /src/crates/memory-mcp/migrations/ /src/crates/memory-mcp/migrations/

ENV HOME=/tmp \
    XDG_DATA_HOME=/tmp/memory-mcp \
    LD_LIBRARY_PATH=/usr/local/lib \
    RUST_LOG=info \
    NER_EXTRACTOR=anno \
    EMBEDDINGS_ENABLED=false \
    SURREALDB_EMBEDDED=false

USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/memory_mcp_http"]
