import XCTest
@testable import Tacie

/// The open edition and the official app from one tree (docs/41 0절, open build split stage 1).
final class BuildTests: XCTestCase {
    func testNamesFourWays() {
        let cases: [(Bool, Bool, BuildNames)] = [
            (false, false, BuildNames(supportFolder: "Tacie", keychainSuffix: "", logFolder: "tacie", mcpName: "tacie")),
            (true, false, BuildNames(supportFolder: "Tacie Dev", keychainSuffix: ".dev", logFolder: "tacie-dev", mcpName: "tacie-dev")),
            (false, true, BuildNames(supportFolder: "Tacie Open", keychainSuffix: ".open", logFolder: "tacie-open", mcpName: "tacie-open")),
            (true, true, BuildNames(supportFolder: "Tacie Open Dev", keychainSuffix: ".open.dev", logFolder: "tacie-open-dev", mcpName: "tacie-open-dev")),
        ]
        for (dev, open, names) in cases {
            XCTAssertEqual(Build.names(dev: dev, open: open), names, "dev \(dev) open \(open)")
        }
    }

    /// One app (#56): sign-in exists only in a build with the official values; a source build has none.
    func testSourceBuildHasNoAccount() {
        let server = URL(string: "https://auth.example")
        XCTAssertFalse(Build.hasAccount(open: true, accountServer: server))
        XCTAssertFalse(Build.hasAccount(open: false, accountServer: nil))
        XCTAssertTrue(Build.hasAccount(open: false, accountServer: server))
    }

    func testEmptyAndUnresolvedValuesAreNil() {
        XCTAssertNil(BuildValues.value("K", in: ["K": ""]))
        XCTAssertNil(BuildValues.value("K", in: ["K": "$(TACIE_X)"]))
        XCTAssertNil(BuildValues.value("K", in: [:]))
        XCTAssertNil(BuildValues.value("K", in: ["K": "  "]))
        XCTAssertEqual(BuildValues.value("K", in: ["K": " v "]), "v")
    }

    func testModelsBaseFallsBack() {
        XCTAssertEqual(BuildValues.modelsBase(in: [:]).absoluteString, "https://models.tacie.app")
        XCTAssertEqual(BuildValues.modelsBase(in: ["TacieModelsBase": "https://m.example"]).absoluteString, "https://m.example")
    }

    /// The model and update-note addresses stay exactly what the shipped app used (hashes and caches key on them).
    func testModelURLsKeepPaths() {
        XCTAssertEqual(ModelManifest.hosted, "https://models.tacie.app/bge-m3/v1/")
        XCTAssertEqual(ModelManifest.voiceBase, "https://models.tacie.app/eres2net-speaker/v1/")
        XCTAssertEqual(ModelManifest.diarBase, "https://models.tacie.app/campp-ft-speaker/v1/")
        XCTAssertEqual(ModelManifest.sttBase, "https://models.tacie.app/whisper-turbo-q5/v1/")
        XCTAssertEqual(UpdateNotes.page(version: "1.2.3", language: "en")?.absoluteString,
                       "https://models.tacie.app/updates/Tacie-1.2.3.en.html")
    }

    /// A build without the official values never sends to Tacie's servers: the fallback host cannot resolve.
    func testServerFallbackIsUnresolvable() {
        XCTAssertEqual(BuildValues.server(nil).host, "unset.invalid")
        XCTAssertEqual(BuildValues.server(URL(string: "https://auth.tacie.app")).host, "auth.tacie.app")
        XCTAssertEqual(LinearConnect.redirect(server: URL(string: "https://auth.tacie.app")!), "https://auth.tacie.app/linear/callback")
    }
}
