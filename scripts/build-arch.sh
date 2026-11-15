#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)
if [[ -z "$version" ]]; then
  echo 'Не удалось определить версию из Cargo.toml.' >&2
  exit 1
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp packaging/arch/PKGBUILD "$work/PKGBUILD"
tar -czf "$work/veyra-${version}.tar.gz" \
  --transform="s,^,veyra-${version}/," \
  Cargo.toml Cargo.lock veyra.desktop assets src
mkdir -p dist
export PKGDEST="$(pwd)/dist"
(
  cd "$work"
  makepkg --clean --force --nodeps
)
