# Favicons

Put these files at the root of a website and add this to `<head>`:

```html
<link rel="icon" href="/favicon.svg" type="image/svg+xml">
<link rel="icon" href="/favicon.ico" sizes="16x16 32x32 48x48">
<link rel="apple-touch-icon" href="/apple-touch-icon.png">
<link rel="manifest" href="/site.webmanifest">
```

| File | What it is |
| --- | --- |
| `favicon.svg` | The filled rounded-face template (24×16, centered). Black on light tabs, cream on dark tabs (`prefers-color-scheme`) |
| `favicon.ico` | The same template at 16, 32 and 48 (PNG inside), for browsers that cannot read SVG |
| `apple-touch-icon.png` 180 · `icon-192.png` · `icon-512.png` | The 3D app icon, for home screens and PWAs |

At 16–32px the 3D render turns into a smudge, so the favicon uses the template (rule 2 in `../README.md`).
