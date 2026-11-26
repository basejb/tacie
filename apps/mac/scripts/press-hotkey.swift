// Presses ⌃⌥⌘M system-wide (needs Accessibility permission for the process running this script).
import ApplicationServices
let src = CGEventSource(stateID: .hidSystemState)
let flags: CGEventFlags = [.maskControl, .maskAlternate, .maskCommand]
for down in [true, false] {
    let e = CGEvent(keyboardEventSource: src, virtualKey: 46, keyDown: down)  // kVK_ANSI_M
    e?.flags = flags
    e?.post(tap: .cghidEventTap)
}
print(AXIsProcessTrusted() ? "accessibility: trusted" : "accessibility: NOT trusted (the key press may be dropped)")
