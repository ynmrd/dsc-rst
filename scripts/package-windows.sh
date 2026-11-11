#!/usr/bin/env bash
# Cross-compile the Windows release build from Linux and package it with NSIS.
# Requirements: rustup target x86_64-pc-windows-gnu, mingw-w64, cmake, nsis (makensis).
# Output: dist/discord-native-<version>-windows-x64.exe and dist/DiscordNative-Setup-<version>.exe
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET=x86_64-pc-windows-gnu
PROFILE=${PROFILE:-release}
VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')

cargo build --profile "$PROFILE" --target "$TARGET" -p discord-native
EXE="target/$TARGET/$PROFILE/discord-native.exe"
mkdir -p dist
cp "$EXE" "dist/discord-native-$VERSION-windows-x64.exe"
makensis -V2 -DVERSION="$VERSION" -DEXE="../$EXE" -DOUTFILE="../dist/DiscordNative-Setup-$VERSION.exe" installer/discord-native.nsi
( cd dist && sha256sum "discord-native-$VERSION-windows-x64.exe" "DiscordNative-Setup-$VERSION.exe" > SHA256SUMS.txt )
ls -la dist
