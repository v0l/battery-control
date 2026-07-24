ARG TARGETARCH

FROM --platform=linux/${TARGETARCH} rust:bullseye as builder

# Install system dependencies
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    pkg-config \
    libdbus-1-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy manifest files first for better caching
COPY Cargo.toml Cargo.lock ./

# Create dummy source to cache dependencies
RUN mkdir -p src && \
    echo "pub fn dummy() {}" > src/lib.rs && \
    cargo build --release --features bluetooth 2>&1 || true

# Copy actual source
COPY src/ ./src/

# Build (without static linking due to proc-macro incompatibility)
RUN cargo build --release --features bluetooth

# Copy only the binary to a minimal output directory
RUN mkdir -p /output && \
    cp /build/target/release/jktool /output/

FROM scratch

COPY --from=builder /output/jktool /jktool

ENTRYPOINT ["/jktool"]
