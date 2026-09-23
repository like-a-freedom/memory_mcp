# syntax=docker/dockerfile:1.7
#
# Builds the two binaries that share the `memory_mcp` package:
#
#   * memory_mcp_http — the Streamable HTTP SaaS server (image entrypoint)
#   * memory_mcp      — the CLI, including the `admin create` / `admin recover`
#                       subcommands (invoked with an entrypoint override, never
#                       through a shell: the runtime is shell-free)
#
# Both are compiled with `streamable-http` — the single coarse switch that
# implies the control plane (OIDC/local-admin), the embedded browser UI, and
# Prometheus — so the local-admin CLI and the bundled UI are present together.
#
# The runtime keeps the pre-existing contract for the non-local modes: the
# distroless nonroot user, no shell, the HTTP entrypoint, the native shared
# libraries, and the compiled migration-asset path.

# ─────────────────────────────────────────────────────────────────────────────
# Stage 1 — control-plane UI bundle (Dioxus 0.7 -> WASM).
#
# VERIFIED: `dx bundle` is run with Dioxus CLI 0.7.10 (the newest 0.7 line, the
# version the crates/control-plane-ui 0.7 dependency resolves against) from the
# workspace root. Running it from crates/control-plane-ui panics:
#
#   dx 0.7.10 -> find_main_package -> std::fs::canonicalize(default_member)
#   unwrap on NotFound, because this workspace declares
#   `default-members = ["crates/memory-mcp"]` and dx resolves those paths
#   relative to the current directory instead of the workspace root.
#
# Invoking from /src with an explicit `--package control-plane-ui` avoids that
# path. The verified output layout is `<out-dir>/public/` (index.html, JS and
# WASM under `public/`), which is why `MEMORY_MCP_CONTROL_PLANE_UI_DIST` below
# points at the `public` subdirectory: `crates/memory-mcp/build.rs` requires a
# non-empty `index.html` at the root of the dist directory. Bump the pinned CLI
# only after re-running `dx --version` / `dx bundle --help` and re-checking this
# layout.
# ─────────────────────────────────────────────────────────────────────────────
FROM rust:1.97.1-slim-trixie AS ui-builder

WORKDIR /src

RUN apt-get update \
    && apt-get install --no-install-recommends --yes \
        ca-certificates \
        cmake \
        libssl-dev \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

ARG DIOXUS_CLI_VERSION=0.7.10
ARG WASM_TARGET=wasm32-unknown-unknown
# Mount prefix for the control-plane bundle (empty = origin root). Must
# equal the path of the deployed MEMORY_MCP_HTTP_PUBLIC_BASE_URL.
ARG MEMORY_MCP_UI_BASE_PATH=

RUN rustup target add "${WASM_TARGET}" \
    && cargo install dioxus-cli --version "${DIOXUS_CLI_VERSION}" --locked

COPY . .

# Absolute, non-symlink output directory containing nonempty index + JS + WASM.
#
# `target/dx` lives inside the cached target mount, so its staging directory
# retains the assets of every earlier build and `dx bundle` copies that
# directory wholesale. Without the clean below the bundle — and the binary that
# embeds it — grows by one stale JS/WASM pair (about 0.8 MB) on every build, and
# ships assets nothing references. The counts are asserted so a regression fails
# the build instead of silently bloating the image.
RUN --mount=type=cache,id=memory-mcp-cargo-registry-ui,target=/usr/local/cargo/registry \
    --mount=type=cache,id=memory-mcp-cargo-git-ui,target=/usr/local/cargo/git \
    --mount=type=cache,id=memory-mcp-target-ui,target=/src/target \
    set -eux; \
    cd /src; \
    rm -rf /src/target/dx/control-plane-ui/release/web/public /src/control-plane-ui-dist; \
    base_args=""; \
    if [ -n "${MEMORY_MCP_UI_BASE_PATH}" ]; then \
        base_args="--base-path ${MEMORY_MCP_UI_BASE_PATH}"; \
    fi; \
    dx bundle --platform web --release --package control-plane-ui --out-dir /src/control-plane-ui-dist ${base_args}; \
    test -s /src/control-plane-ui-dist/public/index.html; \
    test "$(find /src/control-plane-ui-dist/public -type f -name '*.js'   | wc -l)" = "1"; \
    test "$(find /src/control-plane-ui-dist/public -type f -name '*.wasm' | wc -l)" = "1"; \
    test "$(find /src/control-plane-ui-dist/public -type f -name '*.css'  | wc -l)" = "1"

# ─────────────────────────────────────────────────────────────────────────────
# Stage 2 — Rust binaries. Consumes the UI bundle produced by stage 1 and
# embeds it at compile time through `MEMORY_MCP_CONTROL_PLANE_UI_DIST`.
# ─────────────────────────────────────────────────────────────────────────────
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

# The bundle lands at the same literal absolute path `build.rs` reads. Copying
# it into the build stage (rather than pointing at a stage-relative path) keeps
# the contract identical to a host build. The dist directory is copied, not
# symlinked, so `build.rs`'s `symlink_metadata` check passes.
COPY --from=ui-builder /src/control-plane-ui-dist /src/control-plane-ui-dist
ENV MEMORY_MCP_CONTROL_PLANE_UI_DIST=/src/control-plane-ui-dist/public

# `streamable-http` is the profile switch; `mcp-apps` is the orthogonal
# app-session axis that nothing implies, so the image has to name it explicitly.
# This container is the documented way to run the SaaS, so it ships the same
# app-session surface as the release binaries rather than being a different
# product from them.
RUN --mount=type=cache,id=memory-mcp-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=memory-mcp-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=memory-mcp-target-trixie,target=/src/target \
    set -eux; \
    cargo build --locked --release -p memory_mcp --bins \
        --features streamable-http,mcp-apps; \
    mkdir -p /out/runtime; \
    install -Dm755 target/release/memory_mcp /out/memory_mcp; \
    install -Dm755 target/release/memory_mcp_http /out/memory_mcp_http; \
    find target/release -maxdepth 1 -type f \( -name '*.so' -o -name '*.so.*' \) -exec cp -v '{}' /out/runtime/ \;

# The runtime is deliberately shell-free and runs as the unprivileged distroless
# user. Keep this stage free of the Rust toolchain, Cargo registry, and sources.
FROM gcr.io/distroless/cc-debian13:nonroot AS runtime

WORKDIR /app

# `memory_mcp` is the CLI; the image entrypoint stays `memory_mcp_http` so the
# existing HTTP deployments are unchanged. Admin commands run with an
# entrypoint override, e.g.
#   docker run --rm --entrypoint /usr/local/bin/memory_mcp <image> admin create --username ops.one
COPY --from=builder /out/memory_mcp /usr/local/bin/memory_mcp
COPY --from=builder /out/memory_mcp_http /usr/local/bin/memory_mcp_http
COPY --from=builder /out/runtime/ /usr/local/lib/
# Registry migrations currently resolve from the compile-time
# `CARGO_MANIFEST_DIR`; keep those SQL assets at that path in the runtime
# image without shipping the source tree or toolchain. `--chmod` normalizes
# permissions because the runtime user is `nonroot` while some migration files
# are not world-readable in the working tree (e.g. a newly added file created
# with a 002 umask lands as 0660 and would fail startup with `Permission denied`).
COPY --chmod=0755 --from=builder /src/crates/memory-mcp/migrations/ /src/crates/memory-mcp/migrations/

ENV HOME=/tmp \
    XDG_DATA_HOME=/tmp/memory-mcp \
    LD_LIBRARY_PATH=/usr/local/lib \
    RUST_LOG=info \
    NER_EXTRACTOR=anno \
    EMBEDDINGS_ENABLED=false \
    SURREALDB_EMBEDDED=false

USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/memory_mcp_http"]
