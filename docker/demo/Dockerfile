# syntax=docker/dockerfile:1
# Build from the standalone base repository, never the Full workspace.
FROM rust:1.95.0-bookworm AS build
WORKDIR /src
COPY . .
RUN test ! -d modules && test ! -d base
ENV CARGO_PROFILE_DEV_DEBUG=0
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --locked -p talocart -j 2 && cp target/debug/talocart /tmp/talocart

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --create-home talocart \
    && mkdir -p /data && chown talocart:talocart /data
COPY --from=build /tmp/talocart /usr/local/bin/talocart
COPY docker/demo/talocart.toml /etc/talocart.toml
USER talocart
WORKDIR /data
ENTRYPOINT ["talocart", "--config", "/etc/talocart.toml"]
CMD ["serve"]
