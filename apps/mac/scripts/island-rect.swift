// Prints "x,y,w,h" (points, top-left origin) of the largest on-screen Tacie window, for `screencapture -R`.
// An optional argument pads the rectangle on every side.
import CoreGraphics
import Foundation

let pad = Double(CommandLine.arguments.dropFirst().first ?? "0") ?? 0
let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
let rects = windows
    .filter { (($0[kCGWindowOwnerName as String] as? String) ?? "").lowercased() == "tacie" }
    .compactMap { $0[kCGWindowBounds as String] as? [String: Double] }
    .filter { ($0["Height"] ?? 0) > 30 }
    .sorted { ($0["Width"]! * $0["Height"]!) > ($1["Width"]! * $1["Height"]!) }
guard let b = rects.first else { exit(1) }
print("\(Int(b["X"]! - pad)),\(Int(b["Y"]! - pad)),\(Int(b["Width"]! + 2 * pad)),\(Int(b["Height"]! + 2 * pad))")
