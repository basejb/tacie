#!/bin/zsh
# Puts the app's DMG on the public repository's GitHub release (#56, one app): the same signed and notarized DMG the
# landing page serves, built by scripts/release.sh from the private repository. GitHub gets the DMG only, never an
# update feed: the app updates from its own feed (models.tacie.app), and an earlier open-source build reading
# releases/latest/download/appcast.xml must not find one it cannot verify (spec 6절).
# usage: scripts/publish-github.sh [out-dir]   (default build.noindex/release)
# Needs a gh login that can write to the public repository and the tag v<ver> there (scripts/sync-open.py pushes it).
set -e
setopt pipefail
cd "${0:A:h}/.."
repo=basejb/tacie
out=${1:-build.noindex/release}; out=${out:A}
dmgs=("$out"/Tacie-*.dmg(N.om))
dmg=${dmgs[1]:-}
[[ -n "$dmg" ]] || { echo "no Tacie-<ver>.dmg in $out: run scripts/release.sh" >&2; exit 1; }
ver=${${dmg:t:r}#Tacie-}
work=$(mktemp -d); trap 'hdiutil detach -quiet "$work/mnt" 2>/dev/null; rm -rf "$work"' EXIT
hdiutil attach -quiet -nobrowse -readonly -mountpoint "$work/mnt" "$dmg"
id=$(/usr/libexec/PlistBuddy -c "Print CFBundleIdentifier" "$work/mnt/Tacie.app/Contents/Info.plist")
hdiutil detach -quiet "$work/mnt"
[[ "$id" == ai.tacie.app ]] || { echo "$dmg is $id, not the app" >&2; exit 1; }
notes=release-notes/$ver.en.md
[[ -f "$notes" ]] || { echo "no $notes" >&2; exit 1; }
gh api "repos/$repo/git/ref/tags/v$ver" > /dev/null || { echo "no tag v$ver in $repo: run scripts/sync-open.py" >&2; exit 1; }
grep '^- ' "$notes" > "$work/notes.md" || { echo "$notes has no \"- \" lines" >&2; exit 1; }
cp "$dmg" "$work/Tacie.dmg"
if gh release view "v$ver" -R "$repo" > /dev/null 2>&1; then
  gh release upload "v$ver" -R "$repo" --clobber "$work/Tacie.dmg"
else
  gh release create "v$ver" -R "$repo" --verify-tag --latest --title "Tacie $ver" --notes-file "$work/notes.md" "$work/Tacie.dmg"
fi
code=$(curl -s -o /dev/null -w "%{http_code}" -L "https://github.com/$repo/releases/latest/download/Tacie.dmg")
echo "Tacie.dmg: $code"
