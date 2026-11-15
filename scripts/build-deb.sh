#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v dpkg-deb >/dev/null; then
  echo 'Нужен dpkg-deb (пакет dpkg).' >&2
  exit 1
fi

version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)
if [[ -z "$version" ]]; then
  echo 'Не удалось определить версию из Cargo.toml.' >&2
  exit 1
fi

cargo build --release --locked --bins
arch=$(dpkg --print-architecture)
distro=${1:-linux}
if [[ ! "$distro" =~ ^[a-zA-Z0-9.-]+$ ]]; then
  echo 'Недопустимое имя дистрибутива.' >&2
  exit 1
fi
root="dist/own-player_${version}_${arch}"
rm -rf "$root"
install -Dm755 target/release/own-player "$root/usr/bin/own-player"
install -Dm755 target/release/ym-export "$root/usr/bin/ym-export"
install -Dm644 own-player.desktop "$root/usr/share/applications/own-player.desktop"
install -Dm644 assets/own-player.svg "$root/usr/share/icons/hicolor/scalable/apps/own-player.svg"
mkdir -p "$root/DEBIAN"
cat > "$root/DEBIAN/control" <<EOF
Package: own-player
Version: $version
Section: sound
Priority: optional
Architecture: $arch
Maintainer: 68RTF
Depends: libgtk-3-0t64, mpv, ffmpeg, curl
Recommends: libsecret-tools
Description: Local GTK music player powered by mpv
 Local audio player with a queue, lyrics and optional online artwork.
EOF
dpkg-deb --build --root-owner-group "$root" "dist/own-player_${version}_${distro}_${arch}.deb"
