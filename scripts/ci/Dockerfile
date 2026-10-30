# Linux build environment shared by pull-request and tag CI jobs.
#
# Ubuntu 22.04 keeps the glibc 2.35 runtime baseline of published packages.
# The image carries only build inputs: compilers, native build tools, the
# pinned Python and Rust toolchains. The application, the pinned FFmpeg
# libraries, the media corpus and the Cargo build products are still built by
# the jobs that consume this image.
#
# Build from the repository root without sending the working tree as context:
#   docker build -t bili-ci-linux - < scripts/ci/Dockerfile

FROM ubuntu:22.04@sha256:e0f80c829509ef4df9e76bd8944eb7b8ec25cbb379b112a7f9448b2c77a77c9b

ARG DEBIAN_FRONTEND=noninteractive

# Python 3.13.18 is the current security release of the 3.13 series that the
# workflows request; its SHA-256 is the published checksum of
# https://www.python.org/ftp/python/3.13.18/Python-3.13.18.tar.xz
ARG PYTHON_VERSION=3.13.18
ARG PYTHON_SHA256=abb0102e6d4ae027509bcb82327d9dd85515b44a3ba8ad538cfc5e9847029f68

# Rust stays on the toolchain pinned by rust-toolchain.toml.
ARG RUST_VERSION=1.98.1

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      binutils \
      build-essential \
      ca-certificates \
      clang \
      cmake \
      curl \
      git \
      libbz2-dev \
      libclang-dev \
      libffi-dev \
      liblzma-dev \
      libncurses-dev \
      libreadline-dev \
      libsqlite3-dev \
      libssl-dev \
      nasm \
      ninja-build \
      patchelf \
      pkg-config \
      uuid-dev \
      xz-utils \
      zlib1g-dev \
 && rm -rf /var/lib/apt/lists/*

RUN curl -fsSLo /tmp/python.tar.xz \
      "https://www.python.org/ftp/python/${PYTHON_VERSION}/Python-${PYTHON_VERSION}.tar.xz" \
 && echo "${PYTHON_SHA256}  /tmp/python.tar.xz" | sha256sum -c - \
 && mkdir -p /tmp/python-src \
 && tar -xJf /tmp/python.tar.xz -C /tmp/python-src --strip-components=1 \
 && cd /tmp/python-src \
 && ./configure --prefix=/usr/local --with-ensurepip=install > /dev/null \
 && make -j"$(nproc)" > /dev/null \
 && make install > /dev/null \
 && cd / \
 && rm -rf /tmp/python.tar.xz /tmp/python-src \
 && python3 -c "import ssl, sys, tomllib; assert sys.version_info[:3] == tuple(int(part) for part in '${PYTHON_VERSION}'.split('.')); print(sys.version)"

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo
ENV PATH=${CARGO_HOME}/bin:${PATH}

RUN curl -fsSLo /tmp/rustup-init.sh https://sh.rustup.rs \
 && sh /tmp/rustup-init.sh -y --no-modify-path --profile minimal \
      --default-toolchain "${RUST_VERSION}" \
      --component rustfmt --component clippy \
 && rm /tmp/rustup-init.sh \
 && rustc --version \
 && cargo --version \
 && rustfmt --version \
 && cargo clippy --version

WORKDIR /src
