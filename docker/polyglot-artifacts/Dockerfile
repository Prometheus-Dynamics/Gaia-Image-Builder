FROM rust:1.90-bookworm

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        git \
        golang-go \
        maven \
        nodejs \
        npm \
        openjdk-17-jdk \
        python3 \
        python3-pip \
        python3-setuptools \
        python3-wheel \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace
