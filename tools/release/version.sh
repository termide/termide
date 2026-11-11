#!/usr/bin/env bash
#
# The release version, which Cargo.toml's [workspace.package] holds and the
# install instructions, the Nix flake and the AUR packages repeat.
#
#     tools/release/version.sh --check     # every copy matches Cargo.toml
#     tools/release/version.sh 0.50.0      # bump every copy to 0.50.0
#
# The bump rewrites each file that names the current version (CHANGELOG.md,
# changelog.d/ and Cargo.lock aside), resets the AUR pkgrel, refreshes the
# workspace's entries in Cargo.lock, then checks the result. CI runs --check,
# so a copy left behind fails a pull request rather than a release.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# The files that carry the version, with how many times each names it.
EXPECTED=(
    "Cargo.toml:1"
    "flake.nix:2"
    "README.md:23"
    "README.ru.md:23"
    "README.zh.md:23"
    "doc/en/installation.md:16"
    "doc/ru/installation.md:16"
    "doc/zh/installation.md:16"
    "packaging/aur/PKGBUILD:1"
    "packaging/aur/PKGBUILD-bin:1"
)

current() {
    awk '
        /^\[workspace\.package\]/ { in_section = 1; next }
        /^\[/ { in_section = 0 }
        in_section && /^version = "/ { gsub(/version = "|"/, ""); print; exit }
    ' Cargo.toml
}

escape() { printf '%s' "$1" | sed 's/\./\\./g'; }

check() {
    local version escaped failed=0
    version="$(current)"
    escaped="$(escape "$version")"
    for entry in "${EXPECTED[@]}"; do
        local file="${entry%%:*}" want="${entry##*:}" got
        got=$(grep -o "$escaped" "$file" 2>/dev/null | wc -l | tr -d ' ')
        if [ "$got" != "$want" ]; then
            printf 'version: %s names %s %s times, expected %s\n' "$file" "$version" "$got" "$want" >&2
            failed=1
        fi
    done
    # A file not in the list that names the version is a new copy to add.
    while IFS= read -r file; do
        local listed=0
        for entry in "${EXPECTED[@]}"; do
            [ "${entry%%:*}" = "$file" ] && listed=1
        done
        if [ "$listed" -eq 0 ]; then
            printf 'version: %s names %s but is not in tools/release/version.sh\n' "$file" "$version" >&2
            failed=1
        fi
    done < <(git grep -l "$escaped" -- ':!CHANGELOG.md' ':!changelog.d' ':!Cargo.lock' \
        ':!packaging/crates-io-redirect' ':!*.rs' ':!tools/release/version.sh' || true)
    [ "$failed" -eq 0 ] && printf 'version: %s everywhere\n' "$version"
    return "$failed"
}

bump() {
    local new="$1" old escaped
    old="$(current)"
    if ! printf '%s' "$new" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
        printf 'version: %s is not MAJOR.MINOR.PATCH\n' "$new" >&2
        exit 1
    fi
    [ "$new" != "$old" ] || { printf 'version: already %s\n' "$new" >&2; exit 1; }
    escaped="$(escape "$old")"
    for entry in "${EXPECTED[@]}"; do
        local file="${entry%%:*}"
        perl -pi -e "s/$escaped/$new/g" "$file"
    done
    perl -pi -e 's/^pkgrel=.*/pkgrel=1/' packaging/aur/PKGBUILD packaging/aur/PKGBUILD-bin
    cargo update --workspace --quiet
    printf 'version: %s -> %s\n' "$old" "$new"
    check
}

case "${1:-}" in
    --check) check ;;
    "" | -h | --help)
        sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
        ;;
    *) bump "$1" ;;
esac
