#!/bin/sh
# Builds ByteBroom, bundles it as a macOS .app, and installs it to /Applications.
# Usage: ./install.sh   (needs cargo-bundle: cargo install cargo-bundle --locked)
set -e
cd "$(dirname "$0")"

echo "Building and bundling ByteBroom..."
cargo bundle --release

# /Applications is writable by admin users, so no sudo. Remove the old copy first,
# otherwise cp would put the new app inside the old one.
echo "Installing ByteBroom to /Applications..."
rm -rf /Applications/ByteBroom.app
cp -R target/release/bundle/osx/ByteBroom.app /Applications/

# Create a desktop shortcut (optional)
ln -sfn /Applications/ByteBroom.app "$HOME/Desktop/ByteBroom.app"

echo "Installation complete! Open ByteBroom from Launchpad, Spotlight or /Applications."
