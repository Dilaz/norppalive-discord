# Build stage
FROM rust:1.87-slim AS builder

WORKDIR /usr/src/app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    g++ \
    zlib1g-dev \
    python3 \
    make \
    && rm -rf /var/lib/apt/lists/*

# Copy the source code
COPY . .

# Build the application
# It's good practice to build a static binary if possible for distroless, 
# but Rust's default musl target for full static linking can be complex.
# We'll stick to dynamic linking for now, relying on the cc-debian12 base.
RUN cargo build --release

# Runtime stage
FROM gcr.io/distroless/cc-debian12

WORKDIR /usr/local/bin

# Copy the binary from builder
COPY --from=builder /usr/src/app/target/release/norppalive-discord .

# Copy CA certificates from the builder stage
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt

# Set the entrypoint
ENTRYPOINT ["/usr/local/bin/norppalive-discord"]

# It's good practice to run as a non-root user
USER nonroot:nonroot
