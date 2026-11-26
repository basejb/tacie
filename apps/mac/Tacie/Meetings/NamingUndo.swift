import Foundation

extension SavedMeeting {
    /// The engine moved `lines` (named `name` on their own during the meeting) to the voice `voice` (`split`): the
    /// lines are that speaker's now, and a speaker with no name takes `name` (and `person`, a remembered person picked).
    mutating func moved(_ lines: [String], to voice: String, name: String, person: String?) {
        let keys = Set(lines)
        for i in transcript.indices where keys.contains(transcript[i].key) {
            transcript[i].voice = voice
            transcript[i].name = nil
        }
        if voices?.first(where: { $0.key == resolved(voice) })?.name == nil { rename(key: voice, to: name, person: person) }
        // a line of these that the speaker's name does not match keeps its own name
        for i in transcript.indices where keys.contains(transcript[i].key) && displayName(key: voice) != name {
            transcript[i].name = name
        }
    }
}

/// What a naming just did ("화자 2를 해리로 바꿨어요"), shown at the bottom of the meeting page with 되돌리기.
struct NamingToast: Equatable, Identifiable {
    let id = UUID()
    let text: String
}
