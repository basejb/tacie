#!/bin/zsh
# Build Tacie and sign it with a stable identity, so macOS keeps microphone and system-audio permissions
# across rebuilds (an ad-hoc signature changes every build and macOS asks again).
# TACIE_SIGN_IDENTITY picks the identity (SHA-1 or name); default: the first "Apple Development" one.
set -e
setopt pipefail
cd "${0:A:h}/.."
xcodegen generate --quiet
xcodebuild -project Tacie.xcodeproj -scheme Tacie -configuration Debug -derivedDataPath build.noindex build 2>&1 \
  | grep -E "error:|BUILD (SUCCEEDED|FAILED)"
# Finder caches the icon by the bundle's mtime, which xcodebuild leaves alone; bump it so a new icon shows up
touch build.noindex/Build/Products/Debug/Tacie.app
id="${TACIE_SIGN_IDENTITY:-$(security find-identity -v -p codesigning | awk '/Apple Development/ {print $2; exit}')}"
if [[ -n "$id" ]]; then
  codesign --force --deep --sign "$id" build.noindex/Build/Products/Debug/Tacie.app
  echo "signed with ${id[1,8]}…"
else
  echo "no signing identity: ad-hoc build, macOS will ask for permissions after every rebuild"
fi
