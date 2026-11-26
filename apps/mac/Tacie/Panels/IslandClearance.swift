import Foundation

/// Keeping a window clear of the island (the resting pill at the top middle covered the onboarding's Not now on a
/// laptop screen, 2026-11-01).
enum IslandClearance {
    static let gap: CGFloat = 12

    /// The window's origin moved down just enough that its top sits `gap` below the island, never lower than the bottom
    /// of the screen. A window that does not come near the island stays where it is.
    static func origin(clearOf island: NSRect?, window: NSRect, screen: NSRect?) -> NSPoint {
        guard let island, window.intersects(island.insetBy(dx: -gap, dy: -gap)) else { return window.origin }
        var y = island.minY - gap - window.height
        if let screen { y = max(y, screen.minY + 8) }
        return NSPoint(x: window.minX, y: min(window.minY, y))
    }
}
