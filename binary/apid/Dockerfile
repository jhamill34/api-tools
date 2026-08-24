# Builds and ships `apid` for Linux, dynamically linked against a prebuilt,
# pgo+lto-optimized CPython from python-build-standalone (Astral-maintained:
# https://github.com/astral-sh/python-build-standalone). See issue #35 for
# why apid moved off macOS/pyoxidizer.
#
# Build from the repo root so the whole cargo workspace is in context:
#   docker build -f binary/apid/Dockerfile .

# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS builder

# Bump these together when picking up a newer python-build-standalone release:
# https://github.com/astral-sh/python-build-standalone/releases
ARG PYTHON_BUILD_STANDALONE_RELEASE=20260814
ARG PYTHON_VERSION=3.12.14

ARG TARGETARCH

RUN apt-get update && apt-get install -y --no-install-recommends \
    protobuf-compiler \
    libprotobuf-dev \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Map Docker's TARGETARCH to python-build-standalone's target-triple naming.
RUN case "${TARGETARCH}" in \
      amd64) echo "x86_64-unknown-linux-gnu" > /tmp/pbs_triple ;; \
      arm64) echo "aarch64-unknown-linux-gnu" > /tmp/pbs_triple ;; \
      *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac

RUN PBS_TRIPLE="$(cat /tmp/pbs_triple)" && \
    curl -fsSL -o /tmp/python.tar.gz \
      "https://github.com/astral-sh/python-build-standalone/releases/download/${PYTHON_BUILD_STANDALONE_RELEASE}/cpython-${PYTHON_VERSION}%2B${PYTHON_BUILD_STANDALONE_RELEASE}-${PBS_TRIPLE}-install_only_stripped.tar.gz" && \
    mkdir -p /opt/python && \
    tar -xzf /tmp/python.tar.gz -C /opt/python --strip-components=1 && \
    rm /tmp/python.tar.gz

# Picked up by pyo3-build-config so python_runner dynamically links against
# this interpreter instead of whatever (if anything) is on PATH.
ENV PYO3_PYTHON=/opt/python/bin/python3
ENV LD_LIBRARY_PATH=/opt/python/lib

# python-build-standalone's baked-in sysconfig LIBDIR still points at its
# original (non-relocatable) build prefix, which pyo3-build-config trusts
# over `python3-config --ldflags` (which self-relocates correctly) - so the
# linker's -L search path has to be given explicitly here or `cargo build`
# fails at the final link step with "unable to find library -lpython3.12".
ENV RUSTFLAGS="-L /opt/python/lib"

WORKDIR /build
COPY . .

RUN cargo build --release -p apid

# ---- Runtime image ----
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# The interpreter apid was linked against - same absolute path as the build
# stage so its stdlib/landmark discovery keeps working unmodified.
COPY --from=builder /opt/python /opt/python
COPY --from=builder /build/target/release/apid /usr/local/bin/apid

# apid opens its configured log files with File::create, which doesn't
# create parent directories - matches the default etc/apid-config.toml paths.
RUN mkdir -p /var/log/apid

ENV LD_LIBRARY_PATH=/opt/python/lib
ENV APID_CONFIG_PATH=/etc/apid/config.toml

EXPOSE 50051

ENTRYPOINT ["/usr/local/bin/apid"]
