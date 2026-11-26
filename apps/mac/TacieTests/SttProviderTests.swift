import XCTest
@testable import Tacie

/// The open edition's transcription provider (2-B): the values saved and passed to the engine, and the transcription
/// path.
final class SttProviderTests: XCTestCase {
    private let soniox = SttProviderChoice(name: .soniox, region: "jp")
    private let eleven = SttProviderChoice(name: .elevenlabs, region: "us")

    func testPayloadNullWithoutChoice() {
        XCTAssertTrue(SttProviderChoice.payload(choice: nil, key: "k", relay: false) is NSNull)
        XCTAssertTrue(SttProviderChoice.payload(choice: soniox, key: nil, relay: false) is NSNull)
    }

    func testRelayWinsOverProvider() {
        XCTAssertTrue(SttProviderChoice.payload(choice: soniox, key: "k", relay: true) is NSNull)
    }

    func testPayloadSoniox() {
        XCTAssertEqual(SttProviderChoice.payload(choice: soniox, key: "k", relay: false) as? [String: String],
                       ["provider": "soniox", "key": "k", "region": "jp"])
    }

    func testPayloadElevenLabs() {
        XCTAssertEqual(SttProviderChoice.payload(choice: eleven, key: "k", relay: false) as? [String: String],
                       ["provider": "elevenlabs", "key": "k"])
    }

    private func path(_ provider: String?, local: Bool = false, ready: Bool = true, practice: Bool = false) -> SttPath {
        ShadowWiring.path(isDev: false, choice: nil, practice: practice, sample: false, localStt: local,
                          localReady: ready, provider: provider)
    }

    func testNoProviderUnchanged() {
        XCTAssertEqual(path(nil), .stream)
        XCTAssertEqual(path(nil, local: true), .server)
        XCTAssertEqual(path(nil, local: true, ready: false), .server)
    }

    func testElevenLabsIsServer() { XCTAssertEqual(path("elevenlabs"), .server) }
    func testSonioxIsStream() { XCTAssertEqual(path("soniox"), .stream) }
    func testPracticeStaysServer() { XCTAssertEqual(path("soniox", practice: true), .server) }
    func testLocalReadyIsServer() { XCTAssertEqual(path("soniox", local: true, ready: true), .server) }
    func testLocalDownloadingWithSonioxIsStream() { XCTAssertEqual(path("soniox", local: true, ready: false), .stream) }
    func testLocalDownloadingWithElevenLabsIsServer() { XCTAssertEqual(path("elevenlabs", local: true, ready: false), .server) }

    /// 키가 없는 업체는 엔진에 넘기지 않으니 경로도 업체를 따르지 않아요.
    func testPathProviderNeedsKey() {
        XCTAssertNil(SttProviderChoice.pathProvider(choice: soniox, key: nil, relay: false))
        XCTAssertNil(SttProviderChoice.pathProvider(choice: soniox, key: "", relay: false))
        XCTAssertNil(SttProviderChoice.pathProvider(choice: soniox, key: "k", relay: true))
        XCTAssertEqual(SttProviderChoice.pathProvider(choice: eleven, key: "k", relay: false), "elevenlabs")
    }
}
