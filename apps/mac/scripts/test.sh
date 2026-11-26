#!/bin/zsh
# Run the app's unit tests (TacieTests: meeting records, no app host).
set -e
setopt pipefail
cd "${0:A:h}/.."
xcodegen generate --quiet
# TACIE_TEST_RETRY=1 (CI): a failing test runs once more and counts as failed only if it fails again. GitHub's macOS
# runner now and then stretches sleeps or fails an audio-file create that passes on a Mac (2026-11-26).
retry=()
[[ -n "${TACIE_TEST_RETRY:-}" ]] && retry=(-retry-tests-on-failure -test-iterations 2)
xcodebuild test -project Tacie.xcodeproj -scheme TacieTests -destination 'platform=macOS' -derivedDataPath build.noindex "${retry[@]}" 2>&1 \
  | grep -E "error:|failed|TEST (SUCCEEDED|FAILED)|Executed"
