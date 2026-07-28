# ==========================================
# Stage 1: Build binary using Cargo & Rust
# ==========================================
FROM rust:1.85-slim-bookworm AS builder

WORKDIR /app

# Install build-time dependencies (protoc for prost/lance, OpenSSL, build tools)
RUN apt-get update && apt-get install -y --no-install-recommends \
    protobuf-compiler \
    pkg-config \
    libssl-dev \
    ca-certificates \
    curl \
    git \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace manifests and source code
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY src/ src/
COPY templates/ templates/
COPY README.md LICENSE ./

# Build optimized release binary
RUN cargo build --release --bin rms-memory

# ==========================================
# Stage 2: Minimal Runtime Environment
# ==========================================
FROM debian:bookworm-slim AS runner

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    curl \
    git \
    && rm -rf /var/lib/apt/lists/*

# Environment setup
ENV RMS_MEMORY_HOME=/root/.rms-memory
ENV RUST_LOG=info

# Create persistent storage directories
RUN mkdir -p /root/.rms-memory /workspace

# Copy release binary from builder stage
COPY --from=builder /app/target/release/rms-memory /usr/local/bin/rms-memory

# Declare volumes for database persistence and repository mounting
VOLUME ["/root/.rms-memory", "/workspace"]

WORKDIR /workspace

# Standard MCP stdio entrypoint
ENTRYPOINT ["rms-memory"]
CMD ["serve"]
