import Foundation

/// Panel features held back for now (docs/36 0-1절, "뒤로 미룸"): the code stays, the app leaves them off.
/// Call sites read `PanelFeatures.current`; tests switch it to `.allOn` to exercise the held-back code.
@MainActor
struct PanelFeatures: Equatable {
    /// The meeting page's black "다음 회의 준비" button.
    var prepButton = false
    /// "이유 보기" on a decision row.
    var whyLink = false
    /// The sidebar search's "Tacie에게 묻기" row (and Return asking); search is plain meeting filtering without it.
    var searchAsk = false
    /// Linking a meeting to the one it was prepared from: the link on start, the banner, the follow-up and its block.
    var meetingLink = false

    static let allOff = PanelFeatures()
    static let allOn = PanelFeatures(prepButton: true, whyLink: true, searchAsk: true, meetingLink: true)

    static var current = PanelFeatures.allOff
}
