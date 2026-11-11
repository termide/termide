#!/usr/bin/env bash
# Compares the heading skeleton (the level of each `#` heading outside code
# blocks, in order) of every doc/en page with its translations.
#
# doc/ru must keep pace with doc/en (AGENTS.md): a page missing or shaped
# differently fails. doc/zh follows when the change is trivial, so there it
# only warns. Design notes written for developers stay English-only.
#
# Usage: tools/docs/headings.sh
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

english_only=(agent-design.md plugins-design.md)

skeleton() {
  awk '/^[[:space:]]*(```|~~~)/ { code = !code; next }
       !code && /^#+ / { match($0, /^#+/); printf "%d", RLENGTH }' "$1"
}

failed=0
for en in doc/en/*.md; do
  page=$(basename "$en")
  [[ " ${english_only[*]} " == *" $page "* ]] && continue
  want=$(skeleton "$en")
  for lang in ru zh; do
    tr="doc/$lang/$page"
    if [[ ! -f $tr ]]; then
      problem="$tr is missing"
    else
      have=$(skeleton "$tr")
      [[ $have == "$want" ]] && continue
      problem="$tr: headings differ from $en (${#want} in en, ${#have} in $lang)"
    fi
    if [[ $lang == ru ]]; then
      level=error
      failed=1
    else
      level=warning
    fi
    # On GitHub Actions the line becomes an annotation on the run's summary.
    if [[ -n ${GITHUB_ACTIONS:-} ]]; then
      echo "::$level::$problem"
    else
      echo "$level: $problem" >&2
    fi
  done
done

if ((failed)); then
  echo "doc/ru must follow doc/en heading for heading" >&2
  exit 1
fi
echo "headings: doc/ru follows doc/en"
