import Foundation

/// A remembered attendee: someone named in a meeting, kept so the next meeting can know them by voice
/// (design notes 5-4, 6). Only the features needed to know them, never sound.
struct Person: Codable, Equatable, Identifiable {
    let id: String
    var name: String
    var vector: [Float]  // 512 numbers, length 1: every meeting's voice weighted by how long they spoke
    var seconds: Float
    var meetings: Int
    var last: Date  // the latest meeting they were named in
    let color: Int  // index into the speaker palette (SavedMeeting.speakerColors), the same in every meeting
}

/// What the engine handed over for one speaker of a meeting: its voice and how long it spoke.
struct MeetingPrint: Codable, Equatable {
    let key: String
    let channel: String
    let vector: [Float]
    let seconds: Float
    let person: String?  // the remembered person the engine linked this voice to; given back when the session is restored
    /// The voice in the speaker-grouping model (fine-tuned CAM++), when the engine groups with it; given back on restore so a
    /// restarted engine keeps adding to the same speaker. `vector` stays ERes2Net, what remembered people are matched by.
    var diarVector: [Float]? = nil
}
