#!/bin/sh
# Build the termide in this checkout as a static x86_64 Linux binary (musl)
# for the task containers, which are amd64 images: tools/bench/bin/termide.
# Runs in an amd64 Alpine container (Rosetta or QEMU on an arm64 host). The
# toolchain packages go into a local image once, since a slow Alpine mirror
# makes installing them each time take longer than the build; the registry
# and target volumes keep rebuilds incremental.
set -eu
bench=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$bench/../.." && pwd)
image=termide-bench-builder:1.91.1
if ! docker image inspect "$image" >/dev/null 2>&1; then
  docker build --platform linux/amd64 -t "$image" - <<'EOF'
FROM rust:1.91.1-alpine
RUN apk add --no-cache musl-dev g++ make perl cmake linux-headers
EOF
fi
mkdir -p "$bench/bin"
docker run --rm --platform linux/amd64 \
  -v "$root":/src:ro \
  -v termide-bench-cargo:/usr/local/cargo/registry \
  -v termide-bench-target:/target \
  -v "$bench/bin":/out \
  -e CARGO_TARGET_DIR=/target \
  "$image" sh -euc '
    cd /src
    cargo build --release --locked --target x86_64-unknown-linux-musl
    cp /target/x86_64-unknown-linux-musl/release/termide /out/termide
  '
file "$bench/bin/termide"
