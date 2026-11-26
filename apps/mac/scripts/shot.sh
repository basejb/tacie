#!/bin/zsh
# Launch Tacie with a typed question and capture the island (with the screen around it) once the answer is in.
# usage: scripts/shot.sh "question" out.png [wait-seconds]
# For a light backdrop behind it: swift scripts/backdrop.swift page.png & (kill it afterwards)
set -e
cd "${0:A:h}/.."
APP=build.noindex/Build/Products/Debug/Tacie.app
pkill -x Tacie 2>/dev/null || true
open -n "$APP" --args --allow-capture --ask "$1"
sleep "${3:-30}"
rect=$(swift scripts/island-rect.swift 36)
screencapture -x -R "$rect" "$2"
echo "captured $rect -> $2"
