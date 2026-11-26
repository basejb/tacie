import XCTest
@testable import Tacie

final class SpeakerModelTests: XCTestCase {
    private func defaults() -> UserDefaults {
        let name = "speaker-model-\(UUID().uuidString)"
        let d = UserDefaults(suiteName: name)!
        d.removePersistentDomain(forName: name)
        return d
    }

    func testCamppUnlessTheServerSaidEres2net() {
        let d = defaults()
        XCTAssertFalse(SpeakerModel.eres2netOnly(defaults: d, arguments: [], dev: false))  // nothing heard yet
        SpeakerModel.remember("eres2net", defaults: d)
        XCTAssertTrue(SpeakerModel.eres2netOnly(defaults: d, arguments: [], dev: false))
        SpeakerModel.remember(nil, defaults: d)  // an older server: keep the last answer
        XCTAssertTrue(SpeakerModel.eres2netOnly(defaults: d, arguments: [], dev: false))
        SpeakerModel.remember("campp-ft", defaults: d)
        XCTAssertFalse(SpeakerModel.eres2netOnly(defaults: d, arguments: [], dev: false))
    }

    func testADevBuildCanForceOne() {
        let d = defaults()
        XCTAssertTrue(SpeakerModel.eres2netOnly(defaults: d, arguments: ["Tacie", "--speaker-model", "eres2net"], dev: true))
        SpeakerModel.remember("eres2net", defaults: d)
        XCTAssertFalse(SpeakerModel.eres2netOnly(defaults: d, arguments: ["Tacie", "--speaker-model", "campp-ft"], dev: true))
        // the release app ignores the argument
        XCTAssertTrue(SpeakerModel.eres2netOnly(defaults: d, arguments: ["Tacie", "--speaker-model", "campp-ft"], dev: false))
    }

    func testMeDecodesTheSpeakerModel() throws {
        let json = #"{"email": null, "name": null, "picture": null, "status": "active", "month": {"spent_usd": 0, "limit_usd": 1}, "speaker_model": "eres2net"}"#
        XCTAssertEqual(try AccountServer.decoder.decode(AccountInfo.self, from: Data(json.utf8)).speakerModel, "eres2net")
    }
}
