#!/usr/bin/env bash
# Build FilmCraft's own `filmcraft-cli` (unmodified, at the rev the workspace pins) for Android
# and copy it to $1 (the APK's jniLibs/arm64-v8a/libfilmcraft_cli.so). The CLI is a binary-only
# package, so it cannot be a Cargo dependency: it is built from cargo's own git checkout of the
# pinned rev, with that checkout's Cargo.lock, into this repo's target/ directory.
#
# Usage: tools/build-filmcraft-cli.sh <dest .so> [--release]
set -euo pipefail
dest="$1"; shift
profile=debug; flags=()
if [[ "${1:-}" == "--release" ]]; then profile=release; flags=(--release); fi
repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo"
# Fetches the pinned rev if needed, then reports where cargo checked it out.
manifest="$(cargo metadata --format-version 1 | python3 -c 'import json,sys; m=json.load(sys.stdin); print(next(p["manifest_path"] for p in m["packages"] if p["name"]=="filmcraft-automation"))')"
checkout="$(cd "$(dirname "$manifest")/../.." && pwd)"
target="$repo/target/filmcraft-cli"
cargo ndk -t arm64-v8a --platform "${NDK_PLATFORM:-33}" build ${flags[@]+"${flags[@]}"} --locked \
  --manifest-path "$checkout/apps/filmcraft-cli/Cargo.toml" --target-dir "$target" -p filmcraft-cli
mkdir -p "$(dirname "$dest")"
cp "$target/aarch64-linux-android/$profile/filmcraft-cli" "$dest"
echo "filmcraft-cli → $dest"
