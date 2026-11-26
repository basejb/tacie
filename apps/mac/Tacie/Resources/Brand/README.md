# Tacie logo sources

The logo is one rounded face. Width to height is 1.5:1: a wide dome on top and a flat bottom with soft corners. The two
eye lights sit on the widest line (0.58 of the height from the top) at 0.3 and 0.7 of the width. `outline` in
`apps/shared/Sources/TacieShared/BrandGeometry.swift` defines the shape, and the face in the app's floating window uses the same eye spacing. Only
that window's face moves; the logo itself never does.

`character-head.png` is the source render of this shape (1280×720). The app icon is cut from it (see below).

| File | Where it is used |
| --- | --- |
| `l1.svg` | Main logo: website, docs, introductions, store, Slack bot avatar, social previews. Anywhere color is allowed |
| `l4.svg` | One color: print, single-color documents, watermarks. Its color is `currentColor` |
| `template.svg` | Filled template: menu bar, favicon. Always this one below 26px |

Rules
1. The eyes are always at 0.3 and 0.7 of the width, 0.58 of the height from the top (the widest line).
2. Use L1 where color is allowed, L4 where it is not, and the template below 26px.
3. Keep clear space of 0.28 of the face height on every side.
4. The logo does not move. Only the face in the app moves.

To export PNGs, run `build.noindex/Build/Products/Debug/Tacie.app/Contents/MacOS/Tacie --export-brand <folder>`.
It writes `AppIcon-1024.png` (flat L1), `slack-512.png`, `l1-dark-512.png`, `favicon-16/32/48.png` and `sheet.png`.
The app icon (`Tacie/Assets.xcassets/AppIcon.appiconset`) is the 3D render, not the flat logo:
`swift scripts/app-icon-from-render.swift Tacie/Resources/Brand/character-head.png <out-1024.png>` flattens the render's
background to the paper color, cuts the face out along the same outline and centers it in a squircle. `sips` then
scales that 1024 image down to the ten sizes.

For a Slack bot avatar, upload `slack-512.png` in Slack app settings: Basic Information → App icon.

In code, `BrandMark` (SwiftUI) and `BrandImages.menuBarTemplate()` (AppKit) draw from the same geometry
(`BrandGeometry`). These SVG files are copies for use outside the app.
