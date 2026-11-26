import XCTest
@testable import Tacie

final class EngineLaunchTests: XCTestCase {
    private let bundled = URL(fileURLWithPath: "/Applications/Tacie.app/Contents/MacOS/tacie-engine")

    func testOverrideWins() {
        let result = EngineLaunch.executable(env: ["TACIE_ENGINE_BIN": "/dev/engine"], bundled: bundled,
                                             isExecutable: { _ in true })
        XCTAssertEqual(result, .success(URL(fileURLWithPath: "/dev/engine")))
    }

    func testEmptyOverrideUsesBundled() {
        let result = EngineLaunch.executable(env: ["TACIE_ENGINE_BIN": ""], bundled: bundled,
                                             isExecutable: { $0 == self.bundled.path })
        XCTAssertEqual(result, .success(bundled))
        XCTAssertEqual(EngineLaunch.executable(env: [:], bundled: bundled, isExecutable: { _ in true }),
                       .success(bundled))
    }

    func testUnusableOverrideFailsWithoutFallback() {
        // the bundled engine is there and runs, but a broken override must not quietly fall back to it
        let result = EngineLaunch.executable(env: ["TACIE_ENGINE_BIN": "/dev/missing"], bundled: bundled,
                                             isExecutable: { $0 == self.bundled.path })
        XCTAssertEqual(result, .failure(LaunchError(message: "TACIE_ENGINE_BIN이 가리키는 파일을 실행할 수 없어요: /dev/missing")))
    }

    func testNoBundledEngineFails() {
        let missing = LaunchError(message: "앱 안에 검색 엔진이 없어요. 앱을 다시 설치해 주세요.")
        XCTAssertEqual(EngineLaunch.executable(env: [:], bundled: nil, isExecutable: { _ in true }), .failure(missing))
        // a bundle URL that points at nothing runnable counts as missing too
        XCTAssertEqual(EngineLaunch.executable(env: [:], bundled: bundled, isExecutable: { _ in false }),
                       .failure(missing))
    }

    func testArguments() {
        // no checkout picked: no --repo-dir, cards open on GitHub
        let plain = ["--index-dir", "index", "--data-dir", "data", "--exit-on-stdin-eof"]
        XCTAssertEqual(EngineLaunch.arguments(checkout: nil), plain)
        XCTAssertEqual(EngineLaunch.arguments(checkout: nil, sample: "/A/Sample"),
                       ["--index-dir", "index", "--data-dir", "data", "--sample-dir", "/A/Sample", "--exit-on-stdin-eof"])
        XCTAssertEqual(EngineLaunch.arguments(checkout: ""), plain)
        XCTAssertEqual(EngineLaunch.arguments(checkout: "/Users/me/code/tacie"),
                       ["--index-dir", "index", "--data-dir", "data",
                        "--repo-dir", "/Users/me/code/tacie", "--exit-on-stdin-eof"])
    }

    func testSampleFolderFollowsTheLanguage() {
        let both: (String) -> String? = { "/R/\($0)" }
        XCTAssertEqual(EngineLaunch.sampleDir(.korean, find: both), "/R/Sample")
        XCTAssertEqual(EngineLaunch.sampleDir(.english, find: both), "/R/Sample-en")
        // an app built before the English sample existed: English practises on the Korean one
        let koreanOnly: (String) -> String? = { $0 == "Sample" ? "/R/Sample" : nil }
        XCTAssertEqual(EngineLaunch.sampleDir(.english, find: koreanOnly), "/R/Sample")
        XCTAssertNil(EngineLaunch.sampleDir(.korean, find: { _ in nil }))
    }

    func testSampleLanguageFromLocalization() {
        XCTAssertEqual(SampleLanguage.from("en"), .english)
        XCTAssertEqual(SampleLanguage.from("en-GB"), .english)
        XCTAssertEqual(SampleLanguage.from("ko"), .korean)
        XCTAssertEqual(SampleLanguage.from(nil), .korean)
        XCTAssertEqual(SampleLanguage.of("음, 배포는 어떻게 해요?"), .korean)
        XCTAssertEqual(SampleLanguage.of("Um, how do I deploy? Does anyone know?"), .english)
    }

    /// The lines must match PracticeVoice's recording keys character for character (docs/28 PracticeVoice 20–26).
    func testSpokenLine() {
        XCTAssertEqual(SampleLanguage.korean.spokenLine("배포는 어떻게 해요?"), "음, 배포는 어떻게 해요? 혹시 아는 분 있어요?")
        XCTAssertEqual(SampleLanguage.korean.spokenLine("배포는 어떻게 해요"), "음, 배포는 어떻게 해요? 혹시 아는 분 있어요?")
        XCTAssertEqual(SampleLanguage.english.spokenLine("How do I run this project?"),
                       "Um, how do I run this project? Does anyone know?")
        XCTAssertEqual(SampleLanguage.english.spokenLine("Where do API keys go?"),
                       "Um, where do API keys go? Does anyone know?")
        XCTAssertEqual(SampleLanguage.english.spokenLine("API keys go where"), "Um, API keys go where? Does anyone know?")
        XCTAssertEqual(SampleLanguage.english.spokenLine("I can't run it, why"), "Um, I can't run it, why? Does anyone know?")
    }

    /// The practice questions are in their own language, and the English ones are spoken exactly as recorded
    /// (PracticeVoice.recorded). An English app once wrapped the Korean questions: "Um, API 키는 어디에 둬요? Does anyone know?".
    func testPracticeQuestionsFollowTheLanguage() {
        for language in [SampleLanguage.korean, .english] {
            for q in language.practiceQuestions { XCTAssertEqual(SampleLanguage.of(q), language, q) }
        }
        XCTAssertEqual(SampleLanguage.english.practiceQuestions.map(SampleLanguage.english.spokenLine), [
            "Um, how do I run this project? Does anyone know?", "Um, how do I run the tests? Does anyone know?",
            "Um, how do I install it? Does anyone know?", "Um, how do I deploy? Does anyone know?",
            "Um, where are environment variables set? Does anyone know?", "Um, where do API keys go? Does anyone know?",
        ])
    }

    private let home = URL(fileURLWithPath: "/Users/me")
    private let support = URL(fileURLWithPath: "/Users/me/Library/Application Support")

    func testProductBuildKeepsDataInApplicationSupport() {
        // a product build ignores the developer folder setting: it has no repository to live in
        XCTAssertEqual(EngineLaunch.root(env: [:], isDev: false, open: false, saved: "/Users/me/elsewhere", home: home, appSupport: support),
                       URL(fileURLWithPath: "/Users/me/Library/Application Support/Tacie"))
    }

    func testReleaseRootUsesEditionFolder() {
        // the open edition's index and data never land in the official app's folder
        XCTAssertEqual(EngineLaunch.root(env: [:], isDev: false, open: true, saved: nil, home: home, appSupport: support),
                       URL(fileURLWithPath: "/Users/me/Library/Application Support/Tacie Open"))
    }

    func testDevelopmentBuildKeepsTheCheckout() {
        XCTAssertEqual(EngineLaunch.root(env: [:], isDev: true, open: false, saved: nil, home: home, appSupport: support),
                       URL(fileURLWithPath: "/Users/me/Repositories/tacie"))
        XCTAssertEqual(EngineLaunch.root(env: [:], isDev: true, open: false, saved: "/Users/me/other", home: home, appSupport: support),
                       URL(fileURLWithPath: "/Users/me/other"))
    }

    func testTacieRootWinsInBothBuilds() {
        for dev in [true, false] {
            XCTAssertEqual(EngineLaunch.root(env: ["TACIE_ROOT": "/tmp/r"], isDev: dev, saved: "/Users/me/other",
                                             home: home, appSupport: support),
                           URL(fileURLWithPath: "/tmp/r"))
        }
        // an empty TACIE_ROOT is no override
        XCTAssertEqual(EngineLaunch.root(env: ["TACIE_ROOT": ""], isDev: false, open: false, saved: nil, home: home, appSupport: support),
                       URL(fileURLWithPath: "/Users/me/Library/Application Support/Tacie"))
    }
}
