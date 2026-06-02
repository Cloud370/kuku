FROM rust:1.96.0-bookworm

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        clang \
        cmake \
        curl \
        fd-find \
        gh \
        git \
        jq \
        libclang-dev \
        pkg-config \
        python3 \
        ripgrep \
    && ln -sf /usr/bin/fdfind /usr/local/bin/fd \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace

ENV CARGO_TERM_COLOR=always
