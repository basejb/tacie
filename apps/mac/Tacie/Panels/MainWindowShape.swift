import AppKit

/// The main window's title bar: hidden, see-through, 52 pt tall, with the content running under it.
/// Apart from the window controller so tests can build a window of the same shape around one page.
enum MainWindowShape {
    static func apply(_ w: NSWindow) {
        w.styleMask = [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView]
        w.titleVisibility = .hidden
        w.titlebarAppearsTransparent = true
        // an empty unified toolbar makes the title bar 52 pt, so the traffic lights sit centered beside our own row
        w.toolbar = NSToolbar(identifier: "main")
        w.toolbarStyle = .unified
    }
}
