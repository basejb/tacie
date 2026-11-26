#!/bin/zsh
# Packs Tacie.app into a DMG whose Finder window shows the app and 응용 프로그램 on the paper background
# (dmg/background.tiff, drawn from dmg/background.html; issue #8): 640 × 400, 128 pt icons at (180, 196) and
# (460, 196), no toolbar, the app's name without ".app", the app icon on the volume. Signing and notarization
# come later (issue #7, steps 6–7).
# usage: scripts/make-dmg.sh <Tacie.app> <out.dmg> [background.tiff]
set -e
cd "${0:A:h}/.."
app=${1:?usage: scripts/make-dmg.sh <Tacie.app> <out.dmg> [background.tiff]}
out=${2:?usage: scripts/make-dmg.sh <Tacie.app> <out.dmg> [background.tiff]}
app=${app:A}; out=${out:A}; bg=${3:-dmg/background.tiff}
vol=Tacie
link="응용 프로그램"

# Finder finds the window by the volume's name, so a mounted "Tacie" would get this layout instead
if [[ -e /Volumes/$vol ]]; then echo "/Volumes/$vol is mounted; eject it first" >&2; exit 1; fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
stage=$work/stage
mkdir -p "$stage/.background"
cp -R "$app" "$stage/Tacie.app"
SetFile -a E "$stage/Tacie.app"   # Finder shows "Tacie", not "Tacie.app"
ln -s /Applications "$stage/$link"
cp "$bg" "$stage/.background/background.tiff"
cp "$stage/Tacie.app/Contents/Resources/AppIcon.icns" "$stage/.VolumeIcon.icns"

size=$(( $(du -sm "$stage" | cut -f1) + 20 ))
hdiutil create -quiet -srcfolder "$stage" -volname $vol -fs HFS+ -format UDRW -size ${size}m "$work/rw.dmg"
dev=$(hdiutil attach -readwrite -noverify -noautoopen "$work/rw.dmg" | awk '/Apple_HFS/ {print $1}')
SetFile -a C "/Volumes/$vol"   # use .VolumeIcon.icns

# bounds are the whole window: 640 wide, 400 of content under the title bar
# Finder can take a moment to see a fresh mount; asking for the disk before that fails with -1728 (2026-11-10)
for _ in {1..30}; do
  [[ "$(osascript -e "tell application \"Finder\" to exists disk \"$vol\"" 2>/dev/null)" == true ]] && break
  sleep 0.5
done
osascript <<APPLESCRIPT
tell application "Finder"
  tell disk "$vol"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set bounds of container window to {240, 140, 880, 140 + 400 + $(( ${TACIE_DMG_TITLEBAR:-28} ))}
    set opts to icon view options of container window
    set arrangement of opts to not arranged
    set icon size of opts to 128
    set text size of opts to 13
    set background picture of opts to file ".background:background.tiff"
    set position of item "Tacie.app" of container window to {180, 196}
    set position of item "$link" of container window to {460, 196}
    close
    open
    update without registering applications
    delay 1
    close
  end tell
end tell
APPLESCRIPT

sync
hdiutil detach "$dev" -quiet
rm -f "$out"
hdiutil convert -quiet "$work/rw.dmg" -format ULFO -o "$out"
echo "$out"
