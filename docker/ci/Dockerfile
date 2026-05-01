FROM rust:slim-bookworm

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        build-essential \
        ca-certificates \
        git \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace

COPY . .

RUN chmod +x scripts/check-file-sizes.sh scripts/ci.sh

CMD ["bash", "-lc", "cargo fmt --check && ./scripts/check-file-sizes.sh && cargo clippy --workspace --exclude gaia-image-builder --exclude gaia-image-builder-macros --all-targets --all-features -- -D warnings && cargo test --workspace --all-features"]
