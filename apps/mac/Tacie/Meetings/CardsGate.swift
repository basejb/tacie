import Foundation

/// The open edition's in-meeting cards (docs/41 0절, 2-C): answer cards and meeting-end detection run only when there is
/// a key to call Jev with (TypeSafe or OpenRouter). Without a key a meeting starts as "Notes only" (no error is shown),
/// and it cannot switch to answer cards during the meeting. There is no separate on/off switch; the meeting mode
/// (Answer cards · Notes only) is used as is. The official app stays as it is.
enum CardsGate {
    /// `cardsOff`: 직접 고르기의 답 카드 "끄기"(2-D, `OpenAIMeeting.cardsOff`).
    static func startsRecordOnly(practice: Bool, setting: Bool, open: Bool, jev: Bool, cardsOff: Bool = false) -> Bool {
        // 처음 써 보기 연습은 답 카드를 보여 주려고 있어요
        guard !practice else { return false }
        return setting || cardsOff || (open && !jev)
    }

    static func canTurnCardsOn(open: Bool, jev: Bool, cardsOff: Bool = false) -> Bool { !cardsOff && (!open || jev) }

    /// The AI page offers one key, OpenRouter's: TypeSafe lives only in 업체 직접 선택, and naming it here sent people
    /// looking for a key the page never mentions (오픈소스판 e2e, 2026-11-25).
    static let needsKey = String(localized: "답 카드는 OpenRouter 키가 있어야 켜져요")
}

/// 오픈소스판 2-D의 회의 중 동작(AI를 고른 방법에 따라). 공식 앱은 모두 지금 그대로예요.
enum OpenAIMeeting {
    static let chooseLine = String(localized: "받아쓸 방법을 골라 주세요")
    /// Tacie AI is chosen but there is no session (#56): no popup, this line only.
    static let signInLine = String(localized: "로그인하거나 다른 AI를 골라 주세요")
    static let noServerNotes = String(localized: "회의 정리는 설정 → AI에서 모델을 받으면 돼요")
    static let localWaitLine = String(localized: "음성 인식 모델을 받으면 받아써요")

    /// Nothing chosen yet: a missing key means "choose how", not "check your key". A wrong key in `key` mode keeps the
    /// key error (`MeetingModel.say`).
    /// "Mac 안에서만" has no key at all: a key failure there is the speech model still on its way.
    static func troubleLine(kind: String, open: Bool, mode: AIMode?) -> String? {
        guard open, kind == "key" else { return nil }
        switch mode {
        case nil: return chooseLine
        case .tacie: return signInLine
        case .local: return localWaitLine
        default: return nil
        }
    }

    /// A typed question (직접 묻기) whose ranking failed for want of a key: the cards come back in search order and the
    /// line says the answer needs a TypeSafe or OpenRouter key. Nothing chosen and "Mac 안에서만" have no such key; a key
    /// mode or the providers' own keys keep the key error (a wrong key).
    static func askTroubleLine(kind: String, open: Bool, mode: AIMode?) -> String? {
        guard open, kind == "key", mode == nil || mode == .local else { return nil }
        return CardsGate.needsKey
    }

    /// Words for an engine failure in the open edition, before the generic ones (`MeetingModel.say`): nothing chosen
    /// asks to choose (not "check the service key"); a key mode names the OpenRouter key; "Mac 안에서만" never calls an
    /// outside service, so a server that does not answer is the notes model on this Mac (Tacie's own, Ollama or LM Studio).
    /// `keySaved`: an OpenRouter key is in the keychain (API key mode only); without one it asks to add it, not to check it.
    static func failureLine(kind: String, open: Bool, mode: AIMode?, keySaved: Bool = true) -> String? {
        guard open else { return nil }
        switch (kind, mode) {
        case ("key", nil): return chooseInSettings
        case ("key", .key?): return keySaved ? String(localized: "OpenRouter 키를 확인해 주세요") : addKeyInSettings
        case ("down", .local?), ("busy", .local?): return String(localized: "이 Mac의 회의 정리 모델이 응답하지 않아요")
        default: return nil
        }
    }

    static let chooseInSettings = String(localized: "설정 → AI에서 쓸 방법을 골라 주세요")
    static let addKeyInSettings = String(localized: "설정 → AI에서 OpenRouter 키를 넣어 주세요")

    /// The pill as an on-device meeting starts with no model to write its notes (Tacie's own, Ollama or LM Studio).
    static let noModelAtStart = String(localized: "회의 정리에 쓸 모델이 없어요 · 설정 → AI를 확인해 주세요")

    /// "Mac 안에서만" with no notes model found: the notes request is not sent (it would fail as a key error).
    static func notesGo(open: Bool, mode: AIMode?, server: LocalServer?) -> Bool {
        !(open && mode == .local && server == nil)
    }

    static func cardsOff(open: Bool, mode: AIMode?, cards: CardsChoice) -> Bool {
        open && mode == .custom && cards == .off
    }

    @MainActor static var cardsOffNow: Bool { cardsOff(open: AIMode.ownAI, mode: AIMode.saved(), cards: CardsChoice.saved()) }
}

/// The line under 정리 when the notes came back empty (`MeetingPage.emptyNotes`).
enum EmptyNotes {
    static func line(transcribed: Bool, open: Bool, mode: AIMode?) -> String {
        if !transcribed {
            // nothing chosen in the open edition: the meeting was recorded but never transcribed
            return open && mode == nil
                ? String(localized: "AI를 고르지 않아 이 회의는 녹음만 하고 받아쓰지 않았어요. 설정 → AI에서 방법을 고르면 다음 회의부터 받아쓰고 정리해요.")
                : String(localized: "이 회의에서는 받아쓴 말이 없어서 요약을 쓰지 못했어요.")
        }
        return String(localized: "정리할 내용을 찾지 못했어요.")
    }
}
