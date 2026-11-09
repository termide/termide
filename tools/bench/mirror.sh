#!/bin/sh
# A local mirror of what Terminal-Bench verifiers download on every trial:
# the uv binary and Python 3.13 (both from GitHub releases) and pytest with
# its dependencies (PyPI). On a slow or flaky network those downloads fail or
# overrun the verifier's time, and the trial ends with no verdict; served
# from the host they take a second. The tests themselves are unchanged:
# run.sh points the installers here through the verifier's environment.
#
#   tools/bench/mirror.sh fetch   download into mirror/ (skips what is there)
#   tools/bench/mirror.sh serve   serve mirror/ on port $MIRROR_PORT (10001)
#
# The versions are the ones the tasks pin (uv 0.9.5, pytest 8.4.1,
# pytest-json-ctrf 0.3.5) and the Python build uv 0.9.5 picks for `-p 3.13`.
set -eu
bench=$(cd "$(dirname "$0")" && pwd)
mirror="$bench/mirror"
port=${MIRROR_PORT:-10001}

UV_VERSION=0.9.5
PBS_RELEASE=20251014
PYTHON=3.13.9
WHEELS="pytest==8.4.1 pytest-json-ctrf==0.3.5"
# pytest's dependencies on Pythons before 3.11, which pip does not resolve
# for another --python-version.
OLD_PYTHON_WHEELS="exceptiongroup tomli typing_extensions"

get() { # url file
  [ -s "$2" ] && return
  mkdir -p "$(dirname "$2")"
  curl -sSfL --retry 5 --speed-limit 50000 --speed-time 60 -o "$2.part" "$1"
  mv "$2.part" "$2"
  echo "fetched $(basename "$2")"
}

fetch() {
  get "https://github.com/astral-sh/uv/releases/download/$UV_VERSION/uv-x86_64-unknown-linux-gnu.tar.gz" \
    "$mirror/uv/$UV_VERSION/uv-x86_64-unknown-linux-gnu.tar.gz"
  for libc in gnu musl; do
    name="cpython-$PYTHON+$PBS_RELEASE-x86_64-unknown-linux-$libc-install_only_stripped.tar.gz"
    get "https://github.com/astral-sh/python-build-standalone/releases/download/$PBS_RELEASE/$(echo "$name" | sed 's/+/%2B/')" \
      "$mirror/python/$PBS_RELEASE/$name"
  done
  for version in 3.13 3.12 3.11 3.10; do
    # shellcheck disable=SC2086
    python3 -m pip download -q --only-binary=:all: --python-version "$version" \
      --platform manylinux2014_x86_64 --dest "$mirror/wheels" $WHEELS
  done
  # shellcheck disable=SC2086
  python3 -m pip download -q --only-binary=:all: --python-version 3.10 \
    --platform manylinux2014_x86_64 --no-deps --dest "$mirror/wheels" $OLD_PYTHON_WHEELS
  index
}

# A PEP 503 simple index over wheels/: simple/<name>/index.html per project.
index() {
  rm -rf "$mirror/simple"
  for wheel in "$mirror"/wheels/*.whl; do
    file=$(basename "$wheel")
    # The project name is the part before the version; PEP 503 normalises
    # runs of -, _ and . to a single -, in lower case.
    project=$(echo "${file%%-[0-9]*}" | tr 'A-Z' 'a-z' | sed -E 's/[-_.]+/-/g')
    mkdir -p "$mirror/simple/$project"
    echo "<a href=\"../../wheels/$file\">$file</a><br>" >>"$mirror/simple/$project/index.html"
  done
}

case "${1:-}" in
  fetch) fetch ;;
  # OrbStack forwards host.docker.internal to the host's loopback, so the
  # mirror need not be seen from the network. (Elsewhere: MIRROR_BIND.)
  serve) exec python3 -m http.server "$port" --bind "${MIRROR_BIND:-127.0.0.1}" --directory "$mirror" ;;
  *) sed -n '2,12p' "$0" >&2; exit 2 ;;
esac
