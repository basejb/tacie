import XCTest
@testable import Tacie

final class ShadowWiringTests: XCTestCase {
    func testConfigHasTheDocumentedValues() throws {
        let json = try JSONSerialization.jsonObject(with: Data(ShadowWiring.config(terms: ["Tacie", "OpenRouter"], languages: ["ko", "en"]).utf8)) as? [String: Any]
        XCTAssertEqual(json?["model"] as? String, "stt-rt-v6")
        XCTAssertEqual(json?["audio_format"] as? String, "pcm_s16le")
        XCTAssertEqual(json?["sample_rate"] as? Int, 16000)
        XCTAssertEqual(json?["num_channels"] as? Int, 1)
        XCTAssertEqual(json?["language_hints"] as? [String], ["ko", "en"])
        let context = json?["context"] as? [String: Any]
        XCTAssertEqual(context?["terms"] as? [String], ["Tacie", "OpenRouter"])
        let general = context?["general"] as? [[String: String]]
        XCTAssertEqual(general?.first?["key"], "domain")
        XCTAssertEqual(general?.first?["value"], "software team meeting, Korean speech with English technical terms")
    }

    func testOtherLanguagesJoinTheHints() throws {
        let json = try JSONSerialization.jsonObject(with: Data(ShadowWiring.config(terms: [], languages: ["ko", "en", "ja"]).utf8)) as? [String: Any]
        XCTAssertEqual(json?["language_hints"] as? [String], ["ko", "en", "ja"])
        let general = (json?["context"] as? [String: Any])?["general"] as? [[String: String]]
        XCTAssertFalse(general?.first?["value"]?.contains("Korean speech") ?? true)
    }

    func testSpeechLanguagesFollowTheMac() {
        XCTAssertEqual(SpeechLanguages.codes(preferred: ["ko-KR", "en-US"]), ["ko", "en"])
        XCTAssertEqual(SpeechLanguages.codes(preferred: ["ja-JP", "en-JP"]), ["ko", "en", "ja"])
        XCTAssertEqual(SpeechLanguages.codes(preferred: ["zh-Hans-CN", "nb-NO", "fil-PH", "xx"]), ["ko", "en", "zh", "no", "tl"])
        // the Mac's first language, which tells the engine's whisper whether to keep Korean or detect the language
        XCTAssertEqual(SpeechLanguages.first(preferred: ["ko-KR", "en-US"]), "ko")
        XCTAssertEqual(SpeechLanguages.first(preferred: ["en-US", "ko-KR"]), "en")
        XCTAssertEqual(SpeechLanguages.first(preferred: ["nb-NO"]), "no")
        XCTAssertNil(SpeechLanguages.first(preferred: []))
        XCTAssertEqual(SpeechLanguages.codes(preferred: []), ["ko", "en"])
    }

    func testNoTermsMeansNoContextTerms() throws {
        let json = try JSONSerialization.jsonObject(with: Data(ShadowWiring.config(terms: []).utf8)) as? [String: Any]
        XCTAssertNil((json?["context"] as? [String: Any])?["terms"])
    }

    func testTranslationGoesToTheAppLanguage() throws {
        let json = try JSONSerialization.jsonObject(with: Data(ShadowWiring.config(terms: [], translateTo: "ko").utf8)) as? [String: Any]
        let t = json?["translation"] as? [String: String]
        XCTAssertEqual(t?["type"], "one_way")
        XCTAssertEqual(t?["target_language"], "ko")
        let off = try JSONSerialization.jsonObject(with: Data(ShadowWiring.config(terms: []).utf8)) as? [String: Any]
        XCTAssertNil(off?["translation"])
    }

    /// Hidden in a shipped app (docs/30): the switch counts only where the feature is offered; it follows the app's
    /// language, there is no language to pick.
    func testTranslationTarget() {
        XCTAssertNil(MeetingTranslation.target(offered: false, on: true, appKorean: true))
        XCTAssertNil(MeetingTranslation.target(offered: true, on: false, appKorean: true))
        XCTAssertEqual(MeetingTranslation.target(offered: true, on: true, appKorean: true), "ko")
        XCTAssertEqual(MeetingTranslation.target(offered: true, on: true, appKorean: false), "en")
    }

    func testTranslationRidesWithTheTranscriptButNotTheHeader() throws {
        let r = ShadowResult(startMs: 0, endMs: 1000, text: "Got it.", words: [], ms: 90, translation: "알겠어요.", translatedFrom: "en")
        let t = try XCTUnwrap(ShadowWiring.transcript(r))
        XCTAssertEqual(t.translation, "알겠어요.")
        XCTAssertEqual(t.translatedFrom, "en")
        let obj = try XCTUnwrap(JSONSerialization.jsonObject(with: XCTUnwrap(Data(base64Encoded: t.header))) as? [String: Any])
        XCTAssertNil(obj["translation"])
    }

    /// A line the engine joins to the one before keeps both translations.
    func testJoinedLinesKeepBothTranslations() {
        XCTAssertEqual(MeetingTranslation.joined("알겠어요.", "그럼 간격은 그대로 두죠."), "알겠어요. 그럼 간격은 그대로 두죠.")
        XCTAssertEqual(MeetingTranslation.joined(nil, "그럼요."), "그럼요.")
        XCTAssertEqual(MeetingTranslation.joined("네.", nil), "네.")
        XCTAssertNil(MeetingTranslation.joined(nil, nil))
    }

    func testOldTranscriptLinesStillDecode() throws {
        let json = #"{"key":"e1:s1u1","at":0,"text":"네","question":false}"#
        let line = try JSONDecoder().decode(TranscriptLine.self, from: Data(json.utf8))
        XCTAssertNil(line.translation)
        XCTAssertNil(line.translatedFrom)
    }

    func testRecordingStillGetsSamplesWithShadow() {
        let fan = SampleFanout()
        var recorded: [[Float]] = [], shadowed: [[Float]] = []
        fan.recorder = { recorded.append($0) }
        fan.shadow = { shadowed.append($0) }
        fan.feed([0.1, 0.2])
        fan.shadow = nil
        fan.feed([0.3])
        XCTAssertEqual(recorded, [[0.1, 0.2], [0.3]])
        XCTAssertEqual(shadowed, [[0.1, 0.2]])
    }

    func testFanoutCountsStreamMsSinceReset() {
        let fan = SampleFanout()
        fan.feed([Float](repeating: 0, count: 1600))
        XCTAssertEqual(fan.streamMs, 100)
        fan.reset()
        fan.feed([Float](repeating: 0, count: 320))
        XCTAssertEqual(fan.streamMs, 20)
    }
}

final class SttPathTests: XCTestCase {
    func testShippedAppStreams() {
        XCTAssertEqual(ShadowWiring.path(isDev: false, choice: nil, practice: false, sample: false, localStt: false), .stream)
    }

    func testPracticeSampleAndLocalSttKeepTheFilePath() {
        for (p, s, l) in [(true, false, false), (false, true, false), (false, false, true)] {
            XCTAssertEqual(ShadowWiring.path(isDev: false, choice: nil, practice: p, sample: s, localStt: l), .server)
            XCTAssertEqual(ShadowWiring.path(isDev: true, choice: "shadow", practice: p, sample: s, localStt: l), .server)
        }
    }

    func testDevChoices() {
        XCTAssertEqual(ShadowWiring.path(isDev: true, choice: nil, practice: false, sample: false, localStt: false), .stream)
        XCTAssertEqual(ShadowWiring.path(isDev: true, choice: "shadow", practice: false, sample: false, localStt: false), .shadow)
        XCTAssertEqual(ShadowWiring.path(isDev: true, choice: "server", practice: false, sample: false, localStt: false), .server)
        // a shipped app ignores a dev build's leftover choice
        XCTAssertEqual(ShadowWiring.path(isDev: false, choice: "server", practice: false, sample: false, localStt: false), .stream)
    }

    func testStreamTextBecomesTheUtterancesTranscript() {
        let r = ShadowResult(startMs: 5000, endMs: 7000, text: "네 맞아요?", words: [SpokenWord(text: "네", start: 5100, end: 5300),
                                                                                  SpokenWord(text: "맞아요?", start: 5400, end: 6000)], ms: 120)
        let t = ShadowWiring.transcript(r)
        XCTAssertEqual(t?.text, "네 맞아요?")
        XCTAssertEqual(t?.words.map(\.start), [0.1, 0.4])
        XCTAssertEqual(t?.words.map(\.end), [0.3, 1.0])
    }

    func testEmptyStreamTextGoesTheFilePath() {
        XCTAssertNil(ShadowWiring.transcript(ShadowResult(startMs: 0, endMs: 1000, text: "  ", words: [], ms: 90)))
    }

    func testTranscriptHeaderIsBase64JSON() throws {
        let t = StreamTranscript(text: "네", words: [StreamTranscript.Word(text: "네", start: 0.1, end: 0.3)])
        let raw = try XCTUnwrap(Data(base64Encoded: t.header))
        let obj = try XCTUnwrap(JSONSerialization.jsonObject(with: raw) as? [String: Any])
        XCTAssertEqual(obj["text"] as? String, "네")
        let w = try XCTUnwrap((obj["words"] as? [[String: Any]])?.first)
        XCTAssertEqual(w["text"] as? String, "네")
        XCTAssertEqual(w["start"] as? Double, 0.1)
    }

    func testRotateAfterNineMinutes() {
        XCTAssertFalse(ShadowWiring.shouldRotate(ageSeconds: 539))
        XCTAssertTrue(ShadowWiring.shouldRotate(ageSeconds: 540))
    }

    /// A connection that keeps failing (a firewall that blocks only Soniox) must not mint a key every 10 s.
    func testRetriesBackOffAndStop() {
        XCTAssertEqual(ShadowWiring.retryDelay(failures: 0), 10)
        XCTAssertEqual(ShadowWiring.retryDelay(failures: 1), 30)
        XCTAssertEqual(ShadowWiring.retryDelay(failures: 2), 120)
        XCTAssertNil(ShadowWiring.retryDelay(failures: 3))
    }

    /// Every word came after the previous `<fin>`: a short first word that ends before the VAD's late start is still
    /// this utterance's, placed at 0 (2026-11-17).
    func testWordsJustBeforeTheUtteranceAreKept() {
        let r = ShadowResult(startMs: 5000, endMs: 7000, text: "네 그거는", words: [SpokenWord(text: "네", start: 4600, end: 4750),
                                                                              SpokenWord(text: "그거는", start: 5100, end: 5500)], ms: 100)
        let t = ShadowWiring.transcript(r)
        XCTAssertEqual(t?.text, "네 그거는")
        XCTAssertEqual(t?.words.map(\.text), ["네", "그거는"])
        XCTAssertEqual(t?.words.map(\.start), [0, 0.1])
        XCTAssertEqual(t?.words.map(\.end), [0, 0.5])
    }

    /// Something heard long before the utterance is noise the VAD never took for speech: it is still dropped.
    func testStrayWordLongBeforeTheUtteranceIsDropped() {
        let r = ShadowResult(startMs: 10000, endMs: 12000, text: "음 네", words: [SpokenWord(text: "음", start: 6500, end: 6900),
                                                                               SpokenWord(text: "네", start: 10100, end: 10300)], ms: 100)
        let t = ShadowWiring.transcript(r)
        XCTAssertEqual(t?.words.map(\.text), ["네"])
        XCTAssertEqual(t?.text, "네")
    }

    /// Only a stray word: the file path transcribes the WAV.
    func testOnlyAStrayWordGoesTheFilePath() {
        let r = ShadowResult(startMs: 10000, endMs: 12000, text: "음", words: [SpokenWord(text: "음", start: 6500, end: 6900)], ms: 100)
        XCTAssertNil(ShadowWiring.transcript(r))
    }

    /// The VAD's start lags the speech a little: a first word that starts just before it and runs into it stays.
    func testFirstWordAcrossTheStartStays() {
        let r = ShadowResult(startMs: 5000, endMs: 7000, text: "여기까지 할게요.",
                             words: [SpokenWord(text: "여기까지", start: 4800, end: 5300),
                                     SpokenWord(text: "할게요.", start: 5400, end: 5800)], ms: 100)
        let t = ShadowWiring.transcript(r)
        XCTAssertEqual(t?.text, "여기까지 할게요.")
        XCTAssertEqual(t?.words.map(\.start), [0, 0.4])
    }
}

final class SonioxRegionTests: XCTestCase {
    func testKoreaAndAsiaGoToJapan() {
        XCTAssertEqual(SonioxRegion.hint(timeZone: "Asia/Seoul", regionCode: "KR"), "jp")
        XCTAssertEqual(SonioxRegion.hint(timeZone: "Asia/Tokyo", regionCode: "JP"), "jp")
        XCTAssertEqual(SonioxRegion.hint(timeZone: "Australia/Sydney", regionCode: "AU"), "jp")
        // a Korean user travelling keeps Japan
        XCTAssertEqual(SonioxRegion.hint(timeZone: "America/Los_Angeles", regionCode: "KR"), "jp")
    }

    func testEuropeGoesToTheEU() {
        XCTAssertEqual(SonioxRegion.hint(timeZone: "Europe/Berlin", regionCode: "DE"), "eu")
    }

    func testTheRestGoesToTheUS() {
        XCTAssertEqual(SonioxRegion.hint(timeZone: "America/New_York", regionCode: "US"), "us")
        XCTAssertEqual(SonioxRegion.hint(timeZone: "America/Sao_Paulo", regionCode: nil), "us")
    }

    // MARK: invited names as terms (docs/35 3-4)

    func testTermsMergeKeepsEngineFirstAndDedupes() {
        XCTAssertEqual(ShadowWiring.terms(engine: ["Tacie", "정하늘"], names: ["정하늘", "윤가람", "tacie"]),
                       ["Tacie", "정하늘", "윤가람"])
        XCTAssertEqual(ShadowWiring.terms(engine: [], names: []), [])
    }

    func testTermsCapDropsExtraNames() {
        let engine = (1...38).map { "term\($0)" }
        let merged = ShadowWiring.terms(engine: engine, names: ["가", "나", "다", "라"], cap: 40)
        XCTAssertEqual(merged.count, 40)
        XCTAssertEqual(Array(merged.suffix(2)), ["가", "나"])
        // engine terms are never cut for names
        XCTAssertEqual(ShadowWiring.terms(engine: (1...45).map { "t\($0)" }, names: ["가"], cap: 40).count, 45)
    }

    // 2-D: the open edition has no Soniox of its own; without a provider (key mode) or with OpenRouter it transcribes
    // utterances through OpenRouter
    func testOpenWithoutProviderIsServer() {
        XCTAssertEqual(ShadowWiring.path(isDev: false, choice: nil, practice: false, sample: false, localStt: false,
                                         provider: nil, open: true), .server)
    }

    func testOpenOpenRouterIsServer() {
        XCTAssertEqual(ShadowWiring.path(isDev: false, choice: nil, practice: false, sample: false, localStt: false,
                                         provider: "openrouter", open: true), .server)
    }

    func testOfficialWithoutProviderStreams() {
        XCTAssertEqual(ShadowWiring.path(isDev: false, choice: nil, practice: false, sample: false, localStt: false,
                                         provider: nil, open: false), .stream)
    }

    func testOpenSonioxStreams() {
        XCTAssertEqual(ShadowWiring.path(isDev: false, choice: nil, practice: false, sample: false, localStt: false,
                                         provider: "soniox", open: true), .stream)
    }

    // 2026-11-26: switching Settings → AI mid-meeting closes a Soniox connection the new choice would not open
    func testAChoiceThatNoLongerStreamsClosesTheConnection() {
        XCTAssertTrue(ShadowWiring.stopsOnChange(current: .stream, next: .server))
        XCTAssertTrue(ShadowWiring.stopsOnChange(current: .shadow, next: .server))
        XCTAssertFalse(ShadowWiring.stopsOnChange(current: .stream, next: .stream))
        XCTAssertFalse(ShadowWiring.stopsOnChange(current: .server, next: .server))
        XCTAssertFalse(ShadowWiring.stopsOnChange(current: .server, next: .stream))
        // the open edition: Soniox chosen, then on-device processing or an API key
        let soniox = ShadowWiring.path(isDev: false, choice: nil, practice: false, sample: false, localStt: false,
                                       provider: "soniox", open: true)
        let onDevice = ShadowWiring.path(isDev: false, choice: nil, practice: false, sample: false, localStt: true,
                                         provider: nil, open: true)
        let apiKey = ShadowWiring.path(isDev: false, choice: nil, practice: false, sample: false, localStt: false,
                                       provider: nil, open: true)
        XCTAssertEqual(soniox, .stream)
        XCTAssertTrue(ShadowWiring.stopsOnChange(current: soniox, next: onDevice))
        XCTAssertTrue(ShadowWiring.stopsOnChange(current: soniox, next: apiKey))
    }
}
