import Foundation

/// What Soniox heard for one utterance, saved next to the meeting's transcript (dev build only, docs/25).
/// `startMs`/`endMs`: the utterance on the Utterancer's stream clock. `words`: in the recording. `ms`: finalize to `<fin>`.
struct ShadowResult: Codable, Equatable {
    let startMs: Int
    let endMs: Int
    let text: String
    let words: [SpokenWord]
    let ms: Int
    /// The utterance in the app's language when it was said in another one (docs/30); nil without translation.
    var translation: String? = nil
    /// The language it was translated from ("en").
    var translatedFrom: String? = nil
}
