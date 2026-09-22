FROM rust:1.94.0-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY schema.sql ./schema.sql
RUN cargo build --locked --release -j 3

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/eez-indexer /usr/local/bin/eez-indexer
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/eez-indexer"]
