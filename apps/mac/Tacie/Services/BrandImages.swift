import AppKit
import SwiftUI

/// Pictures of the one shape: menu bar template, exported PNGs, and the face at each state for review.
enum BrandImages {
    /// `--export-eyes <dir>`: the island's head at every mood, on the island's paper, at 4× so the lights read.
    /// For looking at the drawing without a screen; the real thing moves.
    @MainActor static func exportEyes(to dir: URL) throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let moods: [(String, EyeMood, Bool, EyeTint)] = [
            ("asleep", .asleep, false, .warm), ("starting", .starting, false, .warm),
            ("listening", .listening, false, .warm), ("listening-remote", .listening, true, .warm),
            ("heard", .heard, false, .warm), ("thinking", .thinking, false, .warm),
            ("happy", .happy, false, .warm), ("unsure", .unsure, false, .warm),
            ("paused", .paused, false, .warm), ("failed", .failed, false, .error),
        ]
        for (name, mood, remote, tint) in moods {
            // settle a rig on the mood so the body channels (tilt, squash) are what the view would show
            var rig = EyeRig()
            rig.mood = mood; rig.remote = remote; rig.tint = tint
            for i in 0..<180 { rig.step(dt: 1.0 / 60, time: Double(i) / 60) }
            let view = HStack(spacing: 10) {
                LitHead(channels: rig.current, tint: tint, height: 24)
                Text(name).font(Theme.chip).foregroundStyle(Theme.ink2)
            }
            .padding(.leading, 10).padding(.trailing, 16)
            .frame(height: Theme.pillHeight)
            .background(Theme.paper.opacity(0.9), in: Capsule())
            .padding(12)
            .background(Color(white: 0.82))
            try write(view, scale: 4, to: dir.appendingPathComponent("eyes-\(name).png"))
        }
    }

    /// `--export-brand <dir>`: the logo for places outside the app. App icon 1024, Slack avatar 512,
    /// favicons 16/32/48 (filled template), L1 on dark 512, plus a review sheet of all three styles.
    @MainActor static func exportBrand(to dir: URL) throws {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try write(appIcon(side: 1024), scale: 1, to: dir.appendingPathComponent("AppIcon-1024.png"))
        // squares: Slack and stores want them. Mark height 237 leaves the 0.28 h clear space on each side.
        try write(BrandMark(style: .l1(dark: false), height: 237).frame(width: 512, height: 512).background(Color(hex: 0xF5F1EA)),
                  scale: 1, to: dir.appendingPathComponent("slack-512.png"))
        try write(BrandMark(style: .l1(dark: true), height: 237).frame(width: 512, height: 512).background(Color(hex: 0x15140F)),
                  scale: 1, to: dir.appendingPathComponent("l1-dark-512.png"))
        for px in [16, 32, 48] {
            try write(BrandMark(style: .template, height: CGFloat(px) / BrandGeometry.aspect).foregroundStyle(Theme.ink)
                        .frame(width: CGFloat(px), height: CGFloat(px)),
                      scale: 1, to: dir.appendingPathComponent("favicon-\(px).png"))
        }
        let sheet = VStack(spacing: 24) {
            HStack(spacing: 32) {
                BrandMark(style: .l1(dark: false), height: 75)
                BrandMark(style: .l4, height: 75).foregroundStyle(Theme.ink)
                BrandMark(style: .template, height: 75).foregroundStyle(Theme.ink)
            }
            HStack(spacing: 32) {
                BrandMark(style: .l1(dark: false), height: 26)
                BrandMark(style: .l4, height: 26).foregroundStyle(Theme.ink)
                BrandMark(style: .template, height: 12.5).foregroundStyle(Theme.ink)
                Image(nsImage: menuBarTemplate())
            }
        }
        .padding(32).background(Color(hex: 0xF5F1EA))
        try write(sheet, scale: 2, to: dir.appendingPathComponent("sheet.png"))
    }

    /// macOS app icon: L1 on a cream squircle, mark 70% of the width.
    private static func appIcon(side: CGFloat) -> some View {
        ZStack {
            RoundedRectangle(cornerRadius: side * 0.2237, style: .continuous)
                .fill(LinearGradient(colors: [Color(hex: 0xFBF8F2), Color(hex: 0xE7DFD0)],
                                     startPoint: .topLeading, endPoint: .bottomTrailing))
                .padding(side * 0.1)   // Apple's icon grid leaves this much around the squircle
            BrandMark(style: .l1(dark: false), height: side * 0.8 * 0.7 / BrandGeometry.aspect)
                .shadow(color: .black.opacity(0.14), radius: side * 0.03, y: side * 0.015)   // lifts the head off the cream
        }
        .frame(width: side, height: side)
    }

    /// Menu bar icon: the filled head with the eyes punched out; macOS tints it.
    static func menuBarTemplate(pointHeight: CGFloat = 14) -> NSImage {
        let h = Double(pointHeight)
        let size = NSSize(width: BrandGeometry.width(forHeight: h), height: h)
        let image = NSImage(size: size, flipped: true) { rect in   // flipped: the outline's y runs from the top
            let path = NSBezierPath(cgPath: BrandGeometry.outline(in: rect))
            let hole = BrandGeometry.holeRadius(height: h)
            let centers = BrandGeometry.eyeCenters(in: rect.size)
            for c in [centers.left, centers.right] {
                path.appendOval(in: NSRect(x: c.x - hole, y: c.y - hole, width: hole * 2, height: hole * 2))
            }
            path.windingRule = .evenOdd
            NSColor.black.setFill()
            path.fill()
            return true
        }
        image.isTemplate = true
        return image
    }

    @MainActor private static func write<V: View>(_ view: V, scale: CGFloat, to url: URL) throws {
        let renderer = ImageRenderer(content: view)
        renderer.scale = scale
        guard let cg = renderer.cgImage else { throw CocoaError(.fileWriteUnknown) }
        let rep = NSBitmapImageRep(cgImage: cg)
        guard let png = rep.representation(using: .png, properties: [:]) else { throw CocoaError(.fileWriteUnknown) }
        try png.write(to: url)
    }
}
