#!/bin/zsh
# Run the app's unit tests (TacieTests: meeting records, no app host).
set -e
setopt pipefail
cd "${0:A:h}/.."
xcodegen generate --quiet
xcodebuild test -project Tacie.xcodeproj -scheme TacieTests -destination 'platform=macOS' -derivedDataPath build.noindex 2>&1 \
  | grep -E "error:|failed|TEST (SUCCEEDED|FAILED)|Executed"
