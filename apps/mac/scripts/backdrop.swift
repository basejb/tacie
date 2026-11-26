// Covers the main screen with an image at normal window level, under the island, until killed.
// For screenshots of how the island looks over a meeting screen: swift scripts/backdrop.swift doc.png &
import AppKit

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
guard let image = NSImage(contentsOfFile: CommandLine.arguments[1]), let screen = NSScreen.main else { exit(1) }
let window = NSWindow(contentRect: screen.frame, styleMask: .borderless, backing: .buffered, defer: false)
let view = NSImageView(frame: NSRect(origin: .zero, size: screen.frame.size))
view.image = image
view.imageScaling = .scaleAxesIndependently
window.contentView = view
window.ignoresMouseEvents = true
window.level = .normal
window.setFrame(screen.frame, display: true)
window.orderFrontRegardless()
app.run()
