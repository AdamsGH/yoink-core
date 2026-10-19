# Experimental images only. They do not replace the Python deployment.
FROM rust:1.88-bookworm AS rust-source
WORKDIR /build
ENV CARGO_BUILD_JOBS=1
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/

FROM rust-source AS launcher-build
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target,sharing=locked \
    CARGO_BUILD_JOBS=1 cargo build --locked --release -p yoink-worker-launcher \
    && cp target/release/yoink-worker-launcher /

FROM rust-source AS rust-build
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target,sharing=locked \
    cargo build --locked --release -p yoink-server -p yoink-worker-launcher \
    && cp target/release/yoink-server target/release/yoink-worker-launcher /

FROM rust-source AS server-config-tests
COPY docs/inline-search.env.example ./docs/inline-search.env.example
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target,sharing=locked \
    cargo test --locked --release -p yoink-server --bin yoink-server config::tests

FROM rust-source AS download-proxy-tests
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target,sharing=locked \
    cargo test --locked --release -p yoink-downloads --test proxy_settings

# Fixture controls are built separately and never copied into the default runtime.
FROM rust-build AS fixture-build
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target,sharing=locked \
    cargo build --locked --release -p yoink-server --features test-fixtures --bin yoink-fixture-server \
    && cp target/release/yoink-fixture-server /

# Operator-only PostgreSQL import utility, not a dependency of yoink-server.
FROM rust-source AS importer-build
COPY schema/ ./schema/
COPY scripts/sql/legacy-catalog.sql ./scripts/sql/legacy-catalog.sql
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target,sharing=locked \
    cargo build --locked --release -p yoink-import \
    && cp target/release/yoink-import /

# Backup/restore is a separate operator binary. Test its safety contracts on Rust 1.88.
FROM rust-source AS operations-build
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target,sharing=locked \
    cargo test --locked -p yoink-ops \
    && cargo build --locked --release -p yoink-ops \
    && cp target/release/yoink-ops /

# Test the separate search component with the pinned compiler and synthetic data.
FROM rust-source AS stats-search-tests
COPY scripts/stats-search-fixture/ ./scripts/stats-search-fixture/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target,sharing=locked \
    cargo test --locked -p yoink-stats-search

FROM python:3.12-slim-bookworm AS worker-runtime
RUN apt-get update \
    && apt-get install --no-install-recommends -y ffmpeg ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && pip install --no-cache-dir yt-dlp==2026.8.19 gallery-dl==1.32.13 readability-lxml==0.8.4.1 trafilatura==2.0.0 httpx==0.28.1 curl-cffi==0.16.3 ytmusicapi==1.12.3 transliterate==1.10.2 mutagen==1.48.1 \
    && groupadd --gid 10001 yoink \
    && useradd --uid 10001 --gid yoink --no-create-home yoink \
    && mkdir -p /tmp/yoink-work \
    && chown yoink:yoink /tmp/yoink-work \
    && chmod 0700 /tmp/yoink-work
COPY worker/ /opt/yoink-worker/
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
USER 10001:10001
WORKDIR /tmp/yoink-work

FROM worker-runtime AS content-worker
COPY --from=launcher-build /yoink-worker-launcher /usr/local/bin/
ENTRYPOINT ["/usr/local/bin/yoink-worker-launcher"]

FROM worker-runtime AS runtime-base
COPY --from=rust-build /yoink-server /usr/local/bin/
COPY --from=rust-build /yoink-worker-launcher /usr/local/bin/
EXPOSE 8088
ENTRYPOINT ["/usr/local/bin/yoink-server"]

# Explicit opt-in test image. This is not the default Docker target.
FROM runtime-base AS media-fixture
COPY --from=fixture-build /yoink-fixture-server /usr/local/bin/
ENTRYPOINT ["/usr/local/bin/yoink-fixture-server"]

# Operator CLI has no Python interpreter, media tools or server entrypoint.
FROM debian:bookworm-slim AS operator-base
RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 yoink \
    && useradd --uid 10001 --gid yoink --no-create-home yoink \
    && mkdir -p /tmp/yoink-work \
    && chown yoink:yoink /tmp/yoink-work \
    && chmod 0700 /tmp/yoink-work
ENV TMPDIR=/tmp/yoink-work TURSO_TMPDIR=/tmp/yoink-work
USER 10001:10001
WORKDIR /tmp/yoink-work

FROM operator-base AS importer
COPY --from=importer-build /yoink-import /usr/local/bin/
ENTRYPOINT ["/usr/local/bin/yoink-import"]

FROM operator-base AS operations
COPY --from=operations-build /yoink-ops /usr/local/bin/
ENTRYPOINT ["/usr/local/bin/yoink-ops"]

FROM runtime-base AS runtime
ARG YOINK_RELEASE_VERSION=0.0.0-dev
ENV YOINK_RUST_RELEASE_VERSION=${YOINK_RELEASE_VERSION}
