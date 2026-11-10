#!/usr/bin/env bash
# Checks the holder on Windows from WSL: copies the crate and the workspace's
# Cargo.lock to a scratch directory under %TEMP%, where the Windows toolchain
# builds it, then runs fmt, clippy and the tests there. Arguments go to
# `cargo test` (for example `-- --nocapture`).
#
# PARVI_HOLDER_SCRATCH names the scratch directory (default parvi-holder), so
# agents working at once don't share one.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
cmd=/mnt/c/Windows/System32/cmd.exe
temp=$(wslpath "$(cd /mnt/c && "$cmd" /c 'echo %TEMP%' | tr -d '\r')")
[[ "$temp" == /mnt/* ]] || { echo "no Windows %TEMP% found through $cmd" >&2; exit 1; }
dir="$temp/${PARVI_HOLDER_SCRATCH:-parvi-holder}"
mkdir -p "$dir"
rsync -a --delete --exclude target --exclude Cargo.lock "$here/" "$dir/"
cp "$root/Cargo.lock" "$dir/Cargo.lock"
cd "$dir"
"$cmd" /c "cd /d $(wslpath -w "$dir") && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test $*"
