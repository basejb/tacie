import Foundation
import Observation

/// Where a source in the panel points: a meeting, and in it a ledger row and/or transcript lines (docs/36 1-2절).
struct PanelTarget: Equatable {
    let meetingID: String
    var keys: [String] = []
    var ledgerIndex: Int? = nil
}

/// What the meeting page shows once it is up: the ledger row lit on 정리, or else the transcript at the lines.
struct PanelMark: Equatable {
    let meetingID: String
    let keys: [String]
    let ledgerIndex: Int?
    /// A second press on the same source lights it again.
    let turn: Int
}

/// The way from a source in the panel to the meeting page (MainSelection holds one). It keeps what the page should light
/// until that page takes it (docs/36 1-2절).
@Observable
@MainActor
final class PanelJump {
    enum Move: Equatable {
        /// The meeting is gone: nothing to open.
        case none
        /// The page already shows it: only the mark.
        case stay
        /// Choose the meeting; `clearQuery` when the list's search hides it (else the page would not change).
        case select(clearQuery: Bool)
    }

    private(set) var mark: PanelMark?
    private var turn = 0

    /// `shown`: the meeting on the page now; `exists`: the meeting is kept; `listed`: the list's search shows it.
    func go(_ t: PanelTarget, shown: String?, exists: Bool, listed: Bool) -> Move {
        guard exists else { return .none }
        turn += 1
        mark = t.keys.isEmpty && t.ledgerIndex == nil ? nil
            : PanelMark(meetingID: t.meetingID, keys: t.keys, ledgerIndex: t.ledgerIndex, turn: turn)
        guard shown != t.meetingID else { return .stay }
        return .select(clearQuery: !listed)
    }

    /// The mark for this meeting's page, once.
    func take(for meetingID: String) -> PanelMark? {
        guard let m = mark, m.meetingID == meetingID else { return nil }
        mark = nil
        return m
    }
}
