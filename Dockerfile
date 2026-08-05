# Raddy — minimal multi-stage image (optional; not required for installation).
# raddy links OpenSSL at runtime, so the slim runtime stage ships libssl3.

FROM rust:1 AS builder
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl-dev pkg-config cmake && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/raddy /usr/local/bin/raddy
ENTRYPOINT ["raddy"]
