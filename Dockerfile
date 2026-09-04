# syntax=docker/dockerfile:1.7
# Public demo image: this service plus the `ecp` binary. Repositories are
# cloned and indexed at runtime, on request, under /data (ephemeral: a
# restart starts empty, which is fine for a demo).
#
#   docker build -t ecp-demo .
#   docker build --build-arg ECP_VERSION=0.14.0 -t ecp-demo .   # release binary
#   docker build --build-arg ECP_REF=<sha> -t ecp-demo .        # build ecp from git

FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY ui ./ui
RUN cargo build --release && install -D target/release/ecp-demo /out/ecp-demo

# `ecp` itself. A release tarball when ECP_VERSION is set (fast, the normal
# path once a release prints the full tool list from `admin mcp tools`);
# otherwise a build of ECP_REF from the ecp repository.
FROM rust:1-slim-bookworm AS ecp
ARG ECP_VERSION=""
ARG ECP_REF=main
RUN apt-get update \
 && apt-get install -y --no-install-recommends build-essential ca-certificates curl git pkg-config \
 && rm -rf /var/lib/apt/lists/*
RUN set -eu; \
    if [ -n "$ECP_VERSION" ]; then \
      curl -sSfL "https://github.com/coseto6125/egent-code-plexus/releases/download/v${ECP_VERSION}/ecp-v${ECP_VERSION}-x86_64-unknown-linux-gnu.tar.gz" | tar -xz -C /tmp; \
      install -D "$(find /tmp -type f -name ecp | head -n 1)" /out/ecp; \
    else \
      cargo install --git https://github.com/coseto6125/egent-code-plexus --rev "$ECP_REF" \
        --bin ecp --root /ecp-root egent-code-plexus; \
      install -D /ecp-root/bin/ecp /out/ecp; \
    fi

FROM debian:bookworm-slim AS runtime
# git clones the repositories and ecp reads HEAD; curl asks the GitHub API
# for a repository's size before the clone.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl git \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --uid 1000 --create-home demo \
 && mkdir -p /data/repos /data/ecp \
 && chown -R demo:demo /data
COPY --from=build /out/ecp-demo /usr/local/bin/
COPY --from=ecp /out/ecp /usr/local/bin/

ENV ECP_HOME=/data/ecp \
    ECP_DEMO_REPOS=/data/repos \
    ECP_NO_TELEMETRY=1 \
    ECP_SKIP_BG_REBUILD=1 \
    PORT=8080

USER demo
WORKDIR /data
EXPOSE 8080
# Liveness is the host's job (`healthCheckPath: /healthz` in render.yaml).
CMD ["ecp-demo"]
