import Foundation

/// What kind of recording the notes were written for (docs/23_녹음_유형별_정리.md): each kind has its own sections.
/// The engine picks one from the start of the transcript; the person can change it on the meeting page.
enum NotesKind: String, Codable, CaseIterable {
    case meeting, interview, talk

    /// The button on the page's date line: the kind's short name, a chevron after it says it opens.
    var button: String { shortName }

    /// The symbol in the kind popover's row.
    var symbol: String {
        switch self {
        case .meeting: "person.3.fill"
        case .interview: "mic.fill"
        case .talk: "person.wave.2.fill"
        }
    }

    /// The name in the button's menu and the confirm dialog.
    var menuTitle: String {
        switch self {
        case .meeting: String(localized: "notesKind.meeting", defaultValue: "회의")
        case .interview: String(localized: "인터뷰·대담")
        case .talk: String(localized: "강의·발표")
        }
    }

    /// The menu's second line: what the notes of this kind hold.
    var menuDetail: String {
        switch self {
        case .meeting: String(localized: "결정 사항, 할 일, 남은 논의")
        case .interview: String(localized: "질문과 답, 핵심 숫자, 기억할 말")
        case .talk: String(localized: "내용 흐름, 핵심 개념, 기억할 점")
        }
    }

    /// The menu name with 으로/로 fitted to its last sound: "인터뷰·대담으로 다시 쓸까요?".
    var asTarget: String {
        switch self {
        case .meeting: String(localized: "회의로")
        case .interview: String(localized: "인터뷰·대담으로")
        case .talk: String(localized: "강의·발표로")
        }
    }

    /// The short name in "인터뷰 형식으로 다시 쓰고 있어요".
    var shortName: String {
        switch self {
        case .meeting: String(localized: "notesKind.meeting", defaultValue: "회의")
        case .interview: String(localized: "인터뷰")
        case .talk: String(localized: "강의")
        }
    }

    /// The short name inside a sentence ("인터뷰 형식으로 …"; English lowercases it: "interview notes").
    var noteName: String {
        switch self {
        case .meeting: String(localized: "notesKind.noteName.meeting", defaultValue: "회의")
        case .interview: String(localized: "notesKind.noteName.interview", defaultValue: "인터뷰")
        case .talk: String(localized: "notesKind.noteName.talk", defaultValue: "강의")
        }
    }

    /// The heading over the topics.
    var flowTitle: String {
        switch self {
        case .meeting: String(localized: "논의 흐름")
        case .interview: String(localized: "notesKind.flow.interview", defaultValue: "질문과 답")
        case .talk: String(localized: "내용 흐름")
        }
    }

    /// The item kinds below the topics, in order, with their headings.
    var sections: [(kind: String, title: String)] {
        switch self {
        case .meeting: [("decision", String(localized: "결정 사항")), ("commitment", String(localized: "할 일")), ("open_question", String(localized: "남은 논의"))]
        case .interview: [("fact", String(localized: "핵심 숫자·사실")), ("quote", String(localized: "기억할 말"))]
        case .talk: [("concept", String(localized: "핵심 개념")), ("takeaway", String(localized: "기억할 점"))]
        }
    }

    /// The line under the skeleton while the notes are being written.
    func writing(rewriting: Bool) -> String {
        if rewriting { return String(localized: "\(noteName) 형식으로 다시 쓰고 있어요. 보통 1분 안에 끝나요.") }
        return switch self {
        case .meeting: String(localized: "요약, 논의 흐름, 결정 사항, 할 일을 쓰고 있어요. 보통 1분 안에 끝나요.")
        case .interview: String(localized: "요약, 질문과 답, 핵심 숫자, 기억할 말을 쓰고 있어요. 보통 1분 안에 끝나요.")
        case .talk: String(localized: "요약, 내용 흐름, 핵심 개념, 기억할 점을 쓰고 있어요. 보통 1분 안에 끝나요.")
        }
    }
}
