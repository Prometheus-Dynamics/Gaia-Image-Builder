FROM rust:slim-bookworm

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        bc \
        bison \
        build-essential \
        bzip2 \
        ca-certificates \
        cpio \
        file \
        flex \
        gawk \
        git \
        gzip \
        make \
        mtools \
        patch \
        perl \
        python3 \
        rsync \
        sed \
        tar \
        unzip \
        wget \
        xz-utils \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace
