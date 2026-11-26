import Foundation

/// Whether AI apps may read this Mac's meetings through the local MCP socket (docs/38 2-2). Two switches must both
/// allow it: the person's own (per account, in settings) and the server's (`/me` field `mcp`: "all", "paid" or "off").
enum MCPAccess {
    /// Open in every build since 2026-11-16 (dev-only before); set false to hide the section again.
    static let offered = true
    static let onKey = "mcpOn"
    static let serverKey = "mcpServer"

    /// The person's switch for one account; off until they turn it on.
    static func isOn(_ d: UserDefaults = .standard, space: AccountSpace = .inUse) -> Bool {
        d.bool(forKey: space.key(onKey))
    }

    static func set(_ on: Bool, _ d: UserDefaults = .standard, space: AccountSpace = .inUse) {
        d.set(on, forKey: space.key(onKey))
    }

    /// Keeps the server's answer (nil: an older server, keep what we had).
    static func remember(_ value: String?, defaults: UserDefaults = .standard) {
        guard let value else { return }
        defaults.set(value, forKey: serverKey)
    }

    /// "off" and "paid" close it; nothing, "all" or a value this app does not know leave it to the person's switch.
    static func serverAllows(_ d: UserDefaults = .standard) -> Bool {
        switch d.string(forKey: serverKey) {
        case "off", "paid": return false
        default: return true
        }
    }

    /// Whether the socket should answer now.
    static func open(_ d: UserDefaults = .standard, space: AccountSpace = .inUse, offered: Bool = MCPAccess.offered) -> Bool {
        offered && isOn(d, space: space) && serverAllows(d)
    }
}
