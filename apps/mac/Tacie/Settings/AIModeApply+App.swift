import Foundation

extension AIModeApply {
    /// Saves the mode and hands the engine what it now means: a new engine when key files flip (`needsRestart`, it
    /// takes the keys once ready), otherwise the keys again; then on-device recognition follows the mode. `custom`
    /// leaves the recognition switch as it is.
    @MainActor static func apply(_ mode: AIMode, server: LocalServer? = nil, context: SettingsContext) async {
        let before = AIMode.saved()
        AIMode.save(mode)
        if mode == .local { LocalServer.save(server) }
        if needsRestart(from: before, to: mode, dev: Build.isDev, open: Build.isOpen) {
            context.engine.restart()
        } else {
            await context.keeper.pushAIKeys()
        }
        switch mode {
        case .local: context.localSTT.turnOn(consented: true, afterSearchModel: true)
        case .key: if context.localSTT.enabled { context.localSTT.turnOff() }
        case .custom: break
        }
    }
}
