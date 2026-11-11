#!/bin/sh
# Check the command nouns advertised by a running node against the skill.
set -eu
skill=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/SKILL.md
nouns=$(awk '/^Commands from the running node / { commands = 1; next } commands && $1 == "parvi" && NF >= 3 && $2 ~ /^[a-z][a-z0-9-]*$/ && $3 ~ /^[a-z][a-z0-9-]*$/ {print $2}' | sort -u)
if [ -z "$nouns" ]; then
    echo "No node command nouns found in help output" >&2
    exit 1
fi
missing=0
for noun in $nouns; do
    # The mission family belongs to the next slice.
    case "$noun" in mission|formation|gate|end|limit|run) continue ;; esac
    if ! grep -Fq "\`$noun\`" "$skill"; then
        echo "Missing noun: $noun" >&2
        missing=1
    fi
done
exit "$missing"
