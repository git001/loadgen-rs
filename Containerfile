# === Builder Stage ===
FROM docker.io/library/rust:1.93-bookworm AS builder

LABEL org.opencontainers.image.source=https://github.com/git001/loadgen-rs
LABEL org.opencontainers.image.description="Load generator written in Rust"
LABEL org.opencontainers.image.licenses="AGPL-3.0"

WORKDIR /build
ARG RUSTFLAGS="-C target-cpu=native"
ARG CARGO_FEATURES=""
ENV RUSTFLAGS="${RUSTFLAGS}"
COPY Cargo.toml Cargo.lock ./
COPY crates/loadgen-ffi/Cargo.toml crates/loadgen-ffi/Cargo.toml
COPY src/ src/
COPY crates/loadgen-ffi/src/ crates/loadgen-ffi/src/

RUN cargo build --release && \
    strip target/release/loadgen-rs

# === curl HTTP/3 Builder Stage (wolfSSL + ngtcp2 + nghttp3) ===
FROM docker.io/library/debian:bookworm-slim AS curl-builder

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        build-essential cmake git ca-certificates \
        pkg-config autoconf automake libtool && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /src

# Build wolfSSL with QUIC support
RUN git clone --depth 1 https://github.com/wolfSSL/wolfssl.git && \
    cd wolfssl && \
    autoreconf -fi && \
    ./configure --prefix=/usr/local \
        --enable-quic --enable-session-ticket \
        --enable-earlydata --enable-psk \
        --enable-harden --enable-altcertchains && \
    make -j$(nproc) && make install && ldconfig

# Build nghttp3
RUN git clone --depth 1 --branch v1.6.0 https://github.com/ngtcp2/nghttp3.git && \
    cd nghttp3 && \
    git submodule update --init --depth 1 && \
    autoreconf -fi && \
    ./configure --prefix=/usr/local --enable-lib-only && \
    make -j$(nproc) && make install && ldconfig

# Build ngtcp2 with wolfSSL
RUN git clone --depth 1 --branch v1.9.1 https://github.com/ngtcp2/ngtcp2.git && \
    cd ngtcp2 && \
    autoreconf -fi && \
    ./configure --prefix=/usr/local --enable-lib-only \
        --with-wolfssl \
        LDFLAGS="-Wl,-rpath,/usr/local/lib" && \
    make -j$(nproc) && make install && ldconfig

# Build curl 8.18.0 with HTTP/3 support via wolfSSL + ngtcp2 + nghttp3
RUN git clone --depth 1 --branch curl-8_18_0 https://github.com/curl/curl.git && \
    cd curl && \
    autoreconf -fi && \
    ./configure --prefix=/usr/local \
        --with-wolfssl \
        --with-nghttp3 \
        --with-ngtcp2 \
        --without-libpsl && \
    make -j$(nproc) && make install && ldconfig

# === Runtime Stage ===
FROM docker.io/library/debian:bookworm-slim

LABEL org.opencontainers.image.source=https://github.com/git001/loadgen-rs
LABEL org.opencontainers.image.description="Load generator written in Rust"
LABEL org.opencontainers.image.licenses="AGPL-3.0"

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy curl with HTTP/3 and its shared libraries
COPY --from=curl-builder /usr/local/bin/curl /usr/local/bin/curl
COPY --from=curl-builder /usr/local/lib/libcurl.so* /usr/local/lib/
COPY --from=curl-builder /usr/local/lib/libnghttp3.so* /usr/local/lib/
COPY --from=curl-builder /usr/local/lib/libngtcp2.so* /usr/local/lib/
COPY --from=curl-builder /usr/local/lib/libngtcp2_crypto_wolfssl.so* /usr/local/lib/
COPY --from=curl-builder /usr/local/lib/libwolfssl.so* /usr/local/lib/
RUN ldconfig

# Copy loadgen-rs binary
COPY --from=builder /build/target/release/loadgen-rs /usr/local/bin/loadgen-rs

ENTRYPOINT ["loadgen-rs"]
CMD ["--help"]

# Build:
#   podman build -t loadgen-rs -f Containerfile .
#
# Run:
#   podman run --rm --network host --cap-add=SYS_ADMIN --security-opt seccomp=unconfined \
#     loadgen-rs -n 1000 -c 10 -t 4 --h1 http://target:8080/
#
# Test curl HTTP/3:
#   podman run --rm --entrypoint curl loadgen-rs --http3 https://example.com/
