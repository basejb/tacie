import Foundation

struct LaunchError: Error, Equatable { let message: String }

/// Which engine binary the app starts and with what flags: kept apart from `EngineProcess` so it can be tested.
/// The engine ships inside the app (`Contents/MacOS/tacie-engine`, put there by `scripts/build-engine.sh`);
/// TACIE_ENGINE_BIN is a developer override. Either way a missing engine is a failure, never a fallback to another one.
enum EngineLaunch {
    static let missingMessage = String(localized: "앱 안에 검색 엔진이 없어요. 앱을 다시 설치해 주세요.")

    /// A non-empty TACIE_ENGINE_BIN wins on its own (unrunnable → failure, not the bundled engine); else the bundled one.
    static func executable(env: [String: String], bundled: URL?,
                           isExecutable: (String) -> Bool) -> Result<URL, LaunchError> {
        if let bin = env["TACIE_ENGINE_BIN"], !bin.isEmpty {
            guard isExecutable(bin) else {
                return .failure(LaunchError(message: String(localized: "TACIE_ENGINE_BIN이 가리키는 파일을 실행할 수 없어요: \(bin)")))
            }
            return .success(URL(fileURLWithPath: bin))
        }
        guard let bundled, isExecutable(bundled.path) else { return .failure(LaunchError(message: missingMessage)) }
        return .success(bundled)
    }

    /// Paths are relative to the working directory (`root`). No --model-dir: the engine reads TACIE_MODEL_DIR from
    /// the inherited environment, else its default folder. --repo-dir only when a development build picked a local
    /// checkout to open cards in (the engine uses it only when its origin is the indexed repository); else cards
    /// open on GitHub.
    static func arguments(checkout: String?, sample: String? = nil) -> [String] {
        var args = ["--index-dir", "index", "--data-dir", "data"]
        if let checkout, !checkout.isEmpty { args += ["--repo-dir", checkout] }
        // the onboarding's sample index inside the app (docs/11 12절)
        if let sample, !sample.isEmpty { args += ["--sample-dir", sample] }
        return args + ["--exit-on-stdin-eof"]
    }

    /// The bundled sample folder for the practice's language (`Contents/Resources/Sample` or `Sample-en`, copied by
    /// `scripts/build-engine.sh`), passed as --sample-dir. English falls back to the Korean sample when its own is missing.
    static func sampleDir(_ language: SampleLanguage, find: (String) -> String?) -> String? {
        language.resources.lazy.compactMap(find).first
    }

    /// The engine's working directory, where its `index` and `data` live. A non-empty TACIE_ROOT wins. A product
    /// build uses the space of the account in use (AccountSpace): `~/Library/Application Support/Tacie/accounts/<id>`
    /// beside that account's meetings, or `~/Library/Application Support/Tacie` signed out. It has no checkout.
    /// A development build keeps the checkout so the index already built there stays in use: the `engineRoot`
    /// setting (developer page), else ~/Repositories/tacie.
    /// A development build keeps one checkout for every account (its search data is the developer's own).
    static func root(env: [String: String], isDev: Bool, open: Bool = Build.isOpen, saved: String?, home: URL,
                     appSupport: URL, space: AccountSpace = .signedOut) -> URL {
        if let r = env["TACIE_ROOT"], !r.isEmpty { return URL(fileURLWithPath: r) }
        guard isDev else {
            return space.folder(base: appSupport.appendingPathComponent(Build.names(dev: false, open: open).supportFolder))
        }
        if let saved, !saved.isEmpty { return URL(fileURLWithPath: saved) }
        // the open edition's development build keeps its own data: never the official development build's checkout
        if open { return appSupport.appendingPathComponent(Build.names(dev: true, open: true).supportFolder) }
        return home.appendingPathComponent("Repositories/tacie")
    }
}

/// The language of the onboarding practice (docs/11 12절): which sample material the engine opens, how the practice
/// meeting's line is worded and which voice says it. English when the app runs in English, else Korean.
enum SampleLanguage: Equatable {
    case korean, english

    static var current: SampleLanguage { from(Bundle.main.preferredLocalizations.first) }

    static func from(_ localization: String?) -> SampleLanguage {
        localization?.lowercased().hasPrefix("en") == true ? .english : .korean
    }

    /// The language a spoken line is in, for the system voice that reads a line without a recording.
    static func of(_ line: String) -> SampleLanguage {
        line.unicodeScalars.contains { (0xAC00...0xD7A3).contains($0.value) || (0x3131...0x318E).contains($0.value) }
            ? .korean : .english
    }

    /// Bundled sample folders, preferred first.
    var resources: [String] { self == .english ? ["Sample-en", "Sample"] : ["Sample"] }

    /// Questions most repositories answer somewhere, in this language, so the practice buttons, the spoken line and
    /// its recording (`PracticeVoice.recorded`) match. A new one needs its recording too.
    var practiceQuestions: [String] {
        switch self {
        case .korean: [
            "이 프로젝트는 어떻게 실행해요?", "테스트는 어떻게 돌려요?", "설치는 어떻게 해요?",
            "배포는 어떻게 해요?", "환경 변수는 어디서 설정해요?", "API 키는 어디에 둬요?",
        ]
        case .english: [
            "How do I run this project?", "How do I run the tests?", "How do I install it?",
            "How do I deploy?", "Where are environment variables set?", "Where do API keys go?",
        ]
        }
    }

    /// The system voice for a line without a recording.
    var voice: String { self == .english ? "en-US" : "ko-KR" }

    /// The practice meeting's line around one of the user's example questions (docs/28 OnboardingModel 161): a filler
    /// before and an ask after, as said in a meeting. The keys of `PracticeVoice.recorded` are lines made here.
    func spokenLine(_ question: String) -> String {
        let q = question.hasSuffix("?") ? question : question + "?"
        switch self {
        case .korean:
            return "음, " + q + " 혹시 아는 분 있어요?"
        case .english:
            // "How do I…" → "how do I…", but "API keys…" and "I…" keep their capitals
            let first = q.prefix { !$0.isWhitespace }
            let keep = first == "I" || first.hasPrefix("I'") || (first.count > 1 && first.prefix(2).allSatisfy(\.isUppercase))
            return "Um, " + (keep ? q : q.prefix(1).lowercased() + q.dropFirst()) + " Does anyone know?"
        }
    }
}
