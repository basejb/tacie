import CoreGraphics
import Foundation

/// When the island window may take its new size, and where it may sit on a screen.
enum IslandSizing {
    static let shrinkDelay: TimeInterval = 0.25

    /// The window's size now and, if it shrinks, after `shrinkDelay`. Each side grows at once and shrinks late, so the
    /// fold (`Theme.exit`) finishes inside the old frame and nothing gets clipped. Per side: opening the ask field from
    /// the chooser makes the pill wider while the card goes, and waiting on the whole size clipped the growing field.
    static func steps(from old: CGSize, to new: CGSize) -> (now: CGSize, later: CGSize?) {
        let now = CGSize(width: max(old.width, new.width), height: max(old.height, new.height))
        return (now, now == new ? nil : new)
    }

    /// Whether the pointer on the pill makes it wider: only the running pill's pause and stop come in. While it does,
    /// the row keeps the width it had before the pointer came (so the island's center holds); otherwise the width
    /// follows the pill, which grows while the pointer rests on it when a question is typed or comes in.
    static func widensOnHover(asking: Bool, closing: Bool, hasQuestion: Bool, failed: Bool, running: Bool) -> Bool {
        !asking && !closing && !hasQuestion && !failed && running
    }

    /// How far to move what the island draws (`drawn`, screen points) so it stays `margin` inside `screen` and never
    /// spills onto the screen next to it. The shadow may hang over the edge. Wider than the screen: centred on it;
    /// taller: its top stays.
    static func push(_ drawn: CGRect, into screen: CGRect, margin: CGFloat) -> CGVector {
        let room = screen.insetBy(dx: margin, dy: margin)
        let dx = drawn.width > room.width ? (room.midX - drawn.midX).rounded() : inside(drawn.minX, drawn.maxX, room.minX, room.maxX)
        let dy = drawn.height > room.height ? room.maxY - drawn.maxY : inside(drawn.minY, drawn.maxY, room.minY, room.maxY)
        return CGVector(dx: dx, dy: dy)
    }

    /// While dragging: past the edge the island follows less and less, `limit` at most, and `push` takes it back on
    /// release.
    static func stretch(_ drawn: CGRect, into screen: CGRect, margin: CGFloat, limit: CGFloat) -> CGVector {
        let p = push(drawn, into: screen, margin: margin)
        let room = screen.insetBy(dx: margin, dy: margin)
        func give(_ d: CGFloat, fits: Bool) -> CGFloat {
            guard fits, d != 0 else { return d }
            return d - (d > 0 ? 1 : -1) * rubber(abs(d), limit: limit)
        }
        return CGVector(dx: give(p.dx, fits: drawn.width <= room.width), dy: give(p.dy, fits: drawn.height <= room.height))
    }

    /// How far past the edge the island shows when dragged `overshoot` past it: one for one at first, never `limit`.
    static func rubber(_ overshoot: CGFloat, limit: CGFloat) -> CGFloat {
        (overshoot * limit / (overshoot + limit)).rounded()
    }

    private static func inside(_ lo: CGFloat, _ hi: CGFloat, _ min: CGFloat, _ max: CGFloat) -> CGFloat {
        if lo < min { return min - lo }
        if hi > max { return max - hi }
        return 0
    }
}
