#!/usr/bin/env bash
#
# Print the changelog fragments under changelog.d/ as the sections of the next
# release: `### Added`, then each entry, in Keep a Changelog order. Entries of
# a section come in the order they were committed, then by name.
#
#     tools/release/changelog-fragments.sh            # print
#     tools/release/changelog-fragments.sh --remove   # print, then git rm them
#
# See changelog.d/README.md.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

remove=0
[ "${1:-}" = "--remove" ] && remove=1

sections=(added changed removed fixed security performance)
titles=(Added Changed Removed Fixed Security Performance)

first=1
all=()
for i in "${!sections[@]}"; do
    dir="changelog.d/${sections[$i]}"
    [ -d "$dir" ] || continue
    entries=()
    while IFS= read -r file; do
        entries+=("$file")
    done < <(
        for file in "$dir"/*.md; do
            [ -e "$file" ] || continue
            # When the fragment was first committed; one not yet committed
            # goes last.
            added=$(git log --diff-filter=A --format=%ct -1 -- "$file" 2>/dev/null)
            printf '%s\t%s\n' "${added:-9999999999}" "$file"
        done | sort -k1,1n -k2,2 | cut -f2
    )
    [ "${#entries[@]}" -gt 0 ] || continue
    [ "$first" -eq 1 ] || printf '\n'
    first=0
    printf '### %s\n\n' "${titles[$i]}"
    for file in "${entries[@]}"; do
        cat "$file"
        all+=("$file")
    done
done

for dir in changelog.d/*/; do
    case " ${sections[*]} " in
        *" $(basename "$dir") "*) ;;
        *) printf 'changelog-fragments: unknown section %s\n' "$dir" >&2; exit 1 ;;
    esac
done

if [ "$remove" -eq 1 ] && [ "${#all[@]}" -gt 0 ]; then
    git rm -q -- "${all[@]}"
fi
