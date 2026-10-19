#!/usr/bin/env bash
# Render the screenshots: ./run.sh [tape-name ...]   (default: every tape)
# Output lands in tools/screenshots/out/. See README.md for what is isolated.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
image=termide-screenshots

[ "${SKIP_BUILD:-}" = 1 ] || DOCKER_BUILDKIT=1 docker build -f "$here/Dockerfile" -t "$image" "$repo"

mkdir -p "$here/out"
if [ $# -eq 0 ]; then
    set -- $(cd "$here/tapes" && ls *.tape | grep -v '^_' | sed 's/\.tape$//') themes
fi

# The theme gallery tape is generated from the built-in theme list.
mkdir -p "$here/out/.gen" "$here/out/themes"
"$here/gen-themes.sh" "$repo/crates/theme/themes" "$here/out/.gen/themes.tape"

failed=()
for tape in "$@"; do
    echo "==> $tape"
    # A tape may ask for extra container options on a `# docker:` line.
    tape_file="tapes/$tape.tape"
    [ "$tape" = themes ] && tape_file="out/.gen/themes.tape"
    extra=$(sed -n 's/^# docker: //p' "$here/$tape_file")
    # No network (loopback still works for the mock model), a fixed hostname,
    # the recording scripts read-only, and the output directory; nothing else
    # from the host is visible inside.
    # shellcheck disable=SC2086
    docker run --rm --network none --hostname demo --tmpfs /tmp:size=2g,exec $extra \
        -v "$here/out:/opt/shots/out" \
        -v "$here/env:/opt/shots/env:ro" \
        -v "$here/fixtures:/opt/shots/fixtures:ro" \
        -v "$here/mock-llm:/opt/shots/mock-llm:ro" \
        -v "$here/tapes:/opt/shots/tapes:ro" \
        "$image" "$tape_file" || { failed+=("$tape"); continue; }
    # Key badges: a tape with `# hint:` lines gets them laid over its GIF
    # (keys.mjs, on this machine: Node, ffmpeg, Chrome), which then replaces
    # the plain one; the MP4 with the badges lands beside it.
    gif=$(sed -n 's|^Output out/\(.*\.gif\)$|\1|p' "$here/$tape_file" | head -1)
    if [ -n "$gif" ] && grep -q '^# hint:' "$here/$tape_file"; then
        if command -v node >/dev/null && command -v ffmpeg >/dev/null; then
            node "$here/keys.mjs" "$here/$tape_file" "$here/out/$gif" \
                --out "$here/out/${gif%.gif}.mp4" --gif >/dev/null || failed+=("$tape (hints)")
        else
            echo "warning: no node or ffmpeg here; out/$gif keeps no key badges" >&2
        fi
    fi
done
# Site file names: out/site/.
docker run --rm --network none -v "$here/out:/opt/shots/out" \
    -v "$here/env:/opt/shots/env:ro" --entrypoint /opt/shots/env/site.sh "$image" \
    || failed+=(site)
echo "done: $here/out"
if [ ${#failed[@]} -gt 0 ]; then
    echo "failed: ${failed[*]}" >&2
    exit 1
fi
