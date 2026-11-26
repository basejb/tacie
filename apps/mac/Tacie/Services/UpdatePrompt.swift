import Foundation

/// When Sparkle's window opens by itself (automatic download off, docs/15): once for a new version, then every 7 days
/// while that version waits. Never during a meeting or in the 30 minutes after one, when its notes open.
enum UpdatePrompt {
    static let every: TimeInterval = 7 * 86400
    static let quietAfterMeeting: TimeInterval = 30 * 60

    static func shows(version: String, now: Date, promptedVersion: String?, promptedAt: Date?,
                      busy: Bool, meetingEndedAt: Date?) -> Bool {
        if busy { return false }
        if let ended = meetingEndedAt, now.timeIntervalSince(ended) < quietAfterMeeting { return false }
        guard promptedVersion == version, let promptedAt else { return true }
        return now.timeIntervalSince(promptedAt) >= every
    }
}

/// When a downloaded version installs by itself (docs/15 7절): Tacie lives in the menu bar and is rarely quit, so a
/// version waiting for 다시 시작 could wait for days. It installs once nobody has touched the Mac for 10 minutes, with
/// no meeting on and the last one ended over 30 minutes ago (its notes, 회의 이어 가기).
enum IdleUpdate {
    static let idleFor: TimeInterval = 10 * 60
    static let checkEvery: TimeInterval = 60
    /// A quiet relaunch is honoured only this soon after it was asked for: an install that never relaunched must not
    /// keep the next ordinary start from opening the main window.
    static let relaunchWithin: TimeInterval = 5 * 60

    static func installs(now: Date, idle: TimeInterval, busy: Bool, holding: Bool, meetingEndedAt: Date?) -> Bool {
        if busy || holding || idle < idleFor { return false }
        if let ended = meetingEndedAt, now.timeIntervalSince(ended) < UpdatePrompt.quietAfterMeeting { return false }
        return true
    }

    /// Back from an idle install: whether to bring the main window up again (`true`: it was open), or nil for an
    /// ordinary start.
    static func reopensMain(askedAt: Date?, mainWasOpen: Bool, now: Date) -> Bool? {
        guard let askedAt, now.timeIntervalSince(askedAt) >= 0, now.timeIntervalSince(askedAt) < relaunchWithin else { return nil }
        return mainWasOpen
    }
}
