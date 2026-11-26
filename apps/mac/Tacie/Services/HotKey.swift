import Carbon.HIToolbox
import Foundation

/// System-wide shortcut through Carbon's RegisterEventHotKey: works without Accessibility permission.
final class HotKey {
    private var ref: EventHotKeyRef?
    private var handler: EventHandlerRef?
    private let action: () -> Void
    private static var actions: [UInt32: () -> Void] = [:]
    private static var nextID: UInt32 = 1

    /// keyCode is a kVK_* virtual key, modifiers are Carbon flags (cmdKey | optionKey ...).
    init(keyCode: Int, modifiers: Int, action: @escaping () -> Void) {
        self.action = action
        let id = HotKey.nextID
        HotKey.nextID += 1
        HotKey.actions[id] = action

        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        InstallEventHandler(GetApplicationEventTarget(), { _, event, _ -> OSStatus in
            var hk = EventHotKeyID()
            GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID),
                              nil, MemoryLayout<EventHotKeyID>.size, nil, &hk)
            if let run = HotKey.actions[hk.id] {
                DispatchQueue.main.async { run() }
            }
            return noErr
        }, 1, &spec, nil, &handler)

        let hotKeyID = EventHotKeyID(signature: OSType(0x7463_7565), id: id)  // 'tcue'
        let status = RegisterEventHotKey(UInt32(keyCode), UInt32(modifiers), hotKeyID, GetApplicationEventTarget(), 0, &ref)
        Log.info(status == noErr ? "hotkey \(keyCode)/\(modifiers) registered" : "hotkey \(keyCode)/\(modifiers) not registered (\(status))")
    }

    deinit {
        if let ref { UnregisterEventHotKey(ref) }
        if let handler { RemoveEventHandler(handler) }
    }
}
