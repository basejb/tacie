import Foundation

extension MeetingLibrary {
    /// Every meeting the engine searches, in a fixed order so an unchanged list is seen as unchanged.
    func digests() -> [MeetingDigest] { meetings.compactMap { $0.digest() }.sorted { $0.id < $1.id } }

    var hasDigest: Bool { meetings.contains(where: \.searchable) }
}

/// Whether a question is searched at all (docs/30): a connected tool, or past meetings to answer from. Only for
/// searching: 팀 자료 in the onboarding and 처음 써 보기 still count connected tools only.
enum SourcePresence {
    static func searchable(facts: Bool, meetings: Bool) -> Bool { facts || meetings }
}
