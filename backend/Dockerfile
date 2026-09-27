# Experimental image only. It does not replace the Python deployment.
FROM rust:1.88-bookworm AS rust-build
WORKDIR /build
ENV CARGO_BUILD_JOBS=4
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --locked --release -p yoink-server \
    && cp target/release/yoink-server /yoink-server

FROM python:3.12-slim-bookworm
RUN apt-get update \
    && apt-get install --no-install-recommends -y ffmpeg ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && pip install --no-cache-dir yt-dlp==2026.8.19 \
    && groupadd --gid 10001 yoink \
    && useradd --uid 10001 --gid yoink --no-create-home yoink \
    && mkdir -p /tmp/yoink-work \
    && chown yoink:yoink /tmp/yoink-work \
    && chmod 0700 /tmp/yoink-work
COPY --from=rust-build /yoink-server /usr/local/bin/yoink-server
COPY worker/ /opt/yoink-worker/
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
USER 10001:10001
WORKDIR /tmp/yoink-work
EXPOSE 8088
ENTRYPOINT ["/usr/local/bin/yoink-server"]
