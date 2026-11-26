#!/bin/sh
# Builds the files attached to a GitHub release: a .dmg (drag to Applications) and a .zip.
# Usage: ./release.sh [arm64|x86_64]   (needs cargo-bundle: cargo install cargo-bundle --locked)
# With no argument it builds for this Mac's chip. Pass the other one to cross-build, e.g.
# ./release.sh x86_64 on an Apple Silicon Mac makes the Intel release (needs rustup target add x86_64-apple-darwin).
# Output goes to dist/. Upload with: gh release create v<version> dist/*
set -e
cd "$(dirname "$0")"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=${1:-$(uname -m)}
case "$arch" in
  arm64) triple=aarch64-apple-darwin ;;
  x86_64) triple=x86_64-apple-darwin ;;
  *) echo "Unknown architecture '$arch'. Use arm64 or x86_64." >&2; exit 1 ;;
esac
name="ByteBroom-$version-macos-$arch"

cargo bundle --release --target "$triple"
app="target/$triple/release/bundle/osx/ByteBroom.app"
# Ad-hoc signature: no Apple account needed. Without one, a downloaded copy is reported
# as "damaged"; with one, macOS only asks the user to allow it once.
codesign --force --deep --sign - "$app"

rm -rf dist && mkdir -p dist/stage
cp -R "$app" dist/stage/
ln -s /Applications dist/stage/Applications
hdiutil create -quiet -volname ByteBroom -srcfolder dist/stage -format UDZO "dist/$name.dmg"
ditto -c -k --keepParent "$app" "dist/$name.zip"
rm -rf dist/stage
ls -lh dist
