/// What a click on the pill does. One meaning in each state, no hidden spots (docs/03 §3): before a meeting it
/// opens the chooser, during one it opens that meeting in the main window, over an answer it opens that answer there.
/// The pill's own buttons (history chip, pause, stop) keep their jobs.
enum PillTap: Equatable {
    case nothing                  // the ask field and the end countdown: only their own buttons act
    case chooser                  // before a meeting, or after one failed to start (the card says why)
    case main(question: String?)  // the main window, at this question when an answer is shown

    static func action(asking: Bool, closing: Bool, current: String?, running: Bool) -> PillTap {
        if asking || closing { return .nothing }
        if let current { return .main(question: current) }
        if running { return .main(question: nil) }
        return .chooser
    }
}
