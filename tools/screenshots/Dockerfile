# Screenshot recorder: builds termide from this checkout, then records VHS
# tapes against a synthetic project in /tmp/demo. The build is glibc, not the
# release's static musl, so libfaketime can pin the date and clock that the
# recordings show (a static binary ignores LD_PRELOAD).
# Build context is the repository root (see run.sh); the matching
# Dockerfile.dockerignore keeps everything but the sources out of it.

# The official Rust image through AWS's public mirror of Docker Hub, which
# needs no Hub login or rate-limit budget. Override with --build-arg.
ARG RUST_IMAGE=public.ecr.aws/docker/library/rust:1.91.1-slim-trixie
FROM ${RUST_IMAGE} AS builder
WORKDIR /src
COPY . .
# A fast release build: the shipped profile's fat LTO and single codegen unit
# only cost time here.
ENV CARGO_PROFILE_RELEASE_LTO=false \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
    CARGO_TARGET_DIR=/cargo-target
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/cargo-target \
    cargo build --release --locked --bin termide \
 && cp /cargo-target/release/termide /termide

FROM ghcr.io/charmbracelet/vhs:v0.12.1
RUN apt-get update \
 && apt-get install -y --no-install-recommends git sqlite3 python3-pylsp python3-debugpy procps locales libcap2-bin faketime \
 && sed -i 's/^# *en_US.UTF-8/en_US.UTF-8/' /etc/locale.gen && locale-gen \
 && rm -rf /var/lib/apt/lists/*
RUN ln -s "$(dpkg -L libfaketime | grep 'libfaketime.so.1$')" /usr/local/lib/libfaketime.so.1
RUN useradd --create-home --uid 1000 --shell /bin/bash demo \
 && setcap cap_sys_admin+ep /usr/bin/unshare
COPY --from=builder /termide /usr/local/bin/termide
COPY tools/screenshots/env/ /opt/shots/env/
COPY tools/screenshots/fixtures/ /opt/shots/fixtures/
COPY tools/screenshots/mock-llm/ /opt/shots/mock-llm/
COPY tools/screenshots/tapes/ /opt/shots/tapes/
RUN install -d -o demo -g demo /opt/shots/out \
 && install -D -o demo -g demo -m 644 /opt/shots/env/bashrc /home/demo/.bashrc \
 && install -D -o demo -g demo -m 644 /opt/shots/env/gitconfig /home/demo/.gitconfig

# Everything a recording can see is fixed here: who, where, when, which language.
ENV HOME=/home/demo USER=demo LOGNAME=demo \
    LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8 TZ=UTC TERM=xterm-256color \
    VHS_NO_SANDBOX=true
USER demo
WORKDIR /opt/shots
ENTRYPOINT ["vhs"]
