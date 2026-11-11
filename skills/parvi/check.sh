#!/bin/sh
# Check every command advertised by a running node against the skill.
set -eu
dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
commands=$(awk '/^Commands from the running node / { commands = 1; next } commands && $1 == "parvi" && NF >= 3 && $2 ~ /^[a-z][a-z0-9-]*$/ && $3 ~ /^[a-z][a-z0-9-]*$/ {print $2, $3}' | sort -u)
if [ -z "$commands" ]; then
    echo "No node commands found in help output" >&2
    exit 1
fi
missing=0
while IFS= read -r command; do
    if ! grep -Fq "\`parvi $command\`" "$dir"/SKILL.md "$dir"/references/*.md; then
        echo "Missing command: parvi $command" >&2
        missing=1
    fi
done <<EOF_COMMANDS
$commands
EOF_COMMANDS
exit "$missing"
