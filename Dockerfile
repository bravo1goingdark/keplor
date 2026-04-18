# --- Build stage: static musl binary ---
FROM rust:1.82-bookworm AS builder

RUN apt-get update && apt-get install -y musl-tools cmake && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates crates
COPY xtask xtask

RUN cargo build --release --locked --target x86_64-unknown-linux-musl -p keplor-cli \
    --features mimalloc \
    && strip target/x86_64-unknown-linux-musl/release/keplor

# --- Runtime stage: scratch + binary ---
FROM alpine:3.20 AS runtime

RUN addgroup -S keplor && adduser -S keplor -G keplor

COPY --from=builder /build/target/x86_64-unknown-linux-musl/release/keplor /usr/local/bin/keplor

RUN mkdir -p /var/lib/keplor && chown keplor:keplor /var/lib/keplor

USER keplor
WORKDIR /var/lib/keplor

EXPOSE 8080

HEALTHCHECK --interval=15s --timeout=3s --start-period=5s --retries=3 \
    CMD wget -qO- http://localhost:8080/health || exit 1

ENTRYPOINT ["keplor"]
CMD ["run", "--config", "/etc/keplor/keplor.toml"]
