import SwiftUI

/// Settings → AI's scene (docs/29 3절, the Meeting tab's Speech recognition scene with OpenRouter on the right): where
/// meeting sound and the transcript go for the open card. API key: on to OpenRouter and back. On-device processing: the
/// light stops at Tacie and OpenRouter is left empty.
struct AIPathScene: View {
    let local: Bool
    /// 키 하나로 with a key that works (or is being checked): the path runs to OpenRouter. Without one it stops at Tacie.
    var keyOn = true
    /// 업체 직접 선택 in use: the right box names the providers that were picked (2026-11-24; the scene was hidden there
    /// before, and the page lost its picture).
    var custom: Custom? = nil
    /// The Tacie AI card is open (#56): the path runs to Tacie AI once signed in. nil: another card.
    var tacie: Bool? = nil
    /// The onboarding's shorter scene: its cards right below already say what the caption would.
    var short = false

    /// What 업체 직접 선택 sends where, in job order (음성 인식, 회의 정리, 답 카드).
    struct Custom: Equatable {
        let stt: AICustomPicks.Stt
        let chat: AICustomPicks.Chat
        let cards: AICustomPicks.Cards

        private var jobs: [(job: String, provider: String?)] {
            let stt: String? = switch self.stt {
            case .openrouter: "OpenRouter"
            case .device: nil
            case .soniox: "Soniox"
            case .elevenlabs: "ElevenLabs"
            }
            let chat: String? = switch self.chat {
            case .openrouter: "OpenRouter"
            case .device: nil
            case .openai: "OpenAI"
            case .other: String(localized: "다른 주소")
            }
            let cards: String? = switch self.cards {
            case .openrouter: "OpenRouter"
            case .typesafe: "TypeSafe"
            case .off: nil
            }
            return [(String(localized: "음성 인식"), stt), (String(localized: "회의 정리"), chat), (String(localized: "답 카드"), cards)]
        }

        /// Every job stays on this Mac (or is off): nothing goes out.
        var onDevice: Bool { jobs.allSatisfy { $0.provider == nil } }
        /// The providers that receive something, without repeats.
        var title: String {
            var seen: [String] = []
            for p in jobs.compactMap(\.provider) where !seen.contains(p) { seen.append(p) }
            return seen.joined(separator: " · ")
        }
        /// The jobs that go out.
        var note: String { jobs.filter { $0.provider != nil }.map(\.job).joined(separator: " · ") }
        /// Each job and where it runs: "음성 인식 Soniox · 회의 정리 이 Mac · 답 카드 꺼짐".
        var caption: String {
            let parts = jobs.enumerated().map { i, j in
                let place = j.provider ?? (i == 2 && cards == .off ? String(localized: "꺼짐") : String(localized: "이 Mac"))
                return "\(j.job) \(place)"
            }
            return parts.joined(separator: " · ")
        }
    }
    @Environment(\.reducesMotion) private var reduceMotion

    private var caption: String {
        if let custom { return custom.caption }
        if let tacie {
            return tacie ? String(localized: "회의 소리와 대화 기록을 Tacie AI로 보내 음성 인식, 회의 정리, 답 카드를 해요.")
                : String(localized: "로그인하면 Tacie AI로 음성 인식, 회의 정리, 답 카드가 켜져요.")
        }
        if local { return String(localized: "회의 소리와 대화 기록을 Mac 밖으로 보내지 않아요. 답 카드는 키가 필요해서 쓸 수 없어요.") }
        if !keyOn { return String(localized: "OpenRouter 키를 넣으면 음성 인식, 회의 정리, 답 카드가 켜져요.") }
        return String(localized: "회의 소리와 대화 기록을 OpenRouter로 보내 음성 인식, 회의 정리, 답 카드를 해요.")
    }

    var body: some View {
        SettingsSceneFrame(caption: caption, short: short) {
            if let custom {
                STTPathScene(onDevice: custom.onDevice,
                             far: .init(title: custom.onDevice ? String(localized: "고른 업체") : custom.title,
                                        note: custom.note, symbol: "slider.horizontal.3"),
                             showsSaid: true)
                    .id(custom.caption)
                    .transition(.opacity)
            } else if let tacie {
                STTPathScene(onDevice: !tacie,
                             far: .init(title: "Tacie AI", note: String(localized: "음성 인식 · 회의 정리 · 답 카드"), symbol: "sparkles",
                                        offNote: String(localized: "로그인하면 켜져요")),
                             showsSaid: tacie)
                    .id("tacie\(tacie)")
                    .transition(.opacity)
            } else {
            STTPathScene(onDevice: local || !keyOn,
                         far: .init(title: "OpenRouter", note: String(localized: "음성 인식 · 회의 정리 · 답 카드"), symbol: "key",
                                    offNote: local ? String(localized: "보내지 않아요") : String(localized: "키를 넣으면 켜져요")),
                         showsSaid: local || keyOn)
                .id("\(local)\(keyOn)")
                .transition(.opacity)
            }
        }
        .animation(Theme.anim(Theme.enter, reduce: reduceMotion), value: local)
        .animation(Theme.anim(Theme.enter, reduce: reduceMotion), value: keyOn)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(caption))
    }
}
