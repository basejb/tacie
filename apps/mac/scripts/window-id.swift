// Prints the CGWindowID of the largest on-screen window owned by Tacie (for `screencapture -l`).
import CoreGraphics
import Foundation

let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
let mine = windows
    .filter { (($0[kCGWindowOwnerName as String] as? String) ?? "").lowercased() == "tacie" }
    .compactMap { w -> (Int, Double)? in
        guard let id = w[kCGWindowNumber as String] as? Int,
              let b = w[kCGWindowBounds as String] as? [String: Double] else { return nil }
        return (id, (b["Width"] ?? 0) * (b["Height"] ?? 0))
    }
    .sorted { $0.1 > $1.1 }
if let first = mine.first { print(first.0) } else { exit(1) }
