FROM rust:1.88-slim AS builder
WORKDIR /usr/src/app
RUN apt-get update && apt-get install -y \
    pkg-config libssl-dev g++ zlib1g-dev protobuf-compiler python3 make \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY proto/ ./proto/
COPY src/ ./src/
COPY build.rs ./
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y \
    libssl3 zlib1g ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 65532 -s /bin/false nonroot
WORKDIR /usr/local/bin
COPY --from=builder /usr/src/app/target/release/norppalive-discord .
USER 65532
ENTRYPOINT ["/usr/local/bin/norppalive-discord"]
