#!/bin/sh
# Scratch-only tracker fixture. Each create records its exact arguments in
# the configured project, then list exposes the created rows to the node.
set -eu
[ "$1" = --json ] && shift
prefix=$(cat .beads/prefix)
case "$1" in
  config)
    printf '{"value":"%s"}\n' "$prefix"
    ;;
  create)
    title=$2
    [ "$3" = --description ]
    description=$4
    [ "$5" = -t ]
    kind=$6
    count=$(cat .beads/count 2>/dev/null || printf 0)
    count=$((count + 1))
    printf '%s' "$count" > .beads/count
    id="$prefix-$count"
    mkdir ".beads/$id"
    printf '%s' "$title" > ".beads/$id/title"
    printf '%s' "$description" > ".beads/$id/description"
    printf '%s' "$kind" > ".beads/$id/type"
    printf '{"id":"%s"}\n' "$id"
    ;;
  list)
    printf '['
    separator=''
    for item in .beads/"$prefix"-*; do
      [ -d "$item" ] || continue
      id=${item##*/}
      title=$(cat "$item/title")
      kind=$(cat "$item/type")
      printf '%s{"id":"%s","title":"%s","issue_type":"%s","status":"open","created_at":"2026-11-01T00:00:00Z","updated_at":"2026-11-01T00:00:00Z"}' "$separator" "$id" "$title" "$kind"
      separator=,
    done
    printf ']\n'
    ;;
  *) printf 'unexpected bd command: %s\n' "$*" >&2; exit 1 ;;
esac
