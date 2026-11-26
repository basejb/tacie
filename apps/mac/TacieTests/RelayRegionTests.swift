import XCTest
@testable import Tacie

final class RelayRegionTests: XCTestCase {
    func testKoreaAndAsiaPacificUseSeoul() {
        XCTAssertEqual(RelayRegion.pick(timeZone: "Asia/Seoul", regionCode: "KR"), .seoul)
        XCTAssertEqual(RelayRegion.pick(timeZone: "Asia/Tokyo", regionCode: "JP"), .seoul)
        XCTAssertEqual(RelayRegion.pick(timeZone: "Australia/Sydney", regionCode: "AU"), .seoul)
        XCTAssertEqual(RelayRegion.pick(timeZone: "Pacific/Auckland", regionCode: "NZ"), .seoul)
    }

    func testAKoreanRegionSettingKeepsSeoulWhileTravelling() {
        XCTAssertEqual(RelayRegion.pick(timeZone: "America/Los_Angeles", regionCode: "KR"), .seoul)
    }

    func testAmericasEuropeAndAfricaUseTheUS() {
        XCTAssertEqual(RelayRegion.pick(timeZone: "America/New_York", regionCode: "US"), .us)
        XCTAssertEqual(RelayRegion.pick(timeZone: "America/Sao_Paulo", regionCode: nil), .us)
        XCTAssertEqual(RelayRegion.pick(timeZone: "Europe/Berlin", regionCode: "DE"), .us)
        XCTAssertEqual(RelayRegion.pick(timeZone: "Africa/Lagos", regionCode: "NG"), .us)
    }

    func testTheEngineAddresses() {
        // the hosts come from the build (Official.xcconfig); a test run has none, so only the paths are checked here
        for r in [RelayRegion.seoul, .us] {
            XCTAssertEqual(AccountServer.openRouterRelay(r), r.root + "/ai/openrouter")
            XCTAssertEqual(AccountServer.jevRelay(r), r.root + "/ai/jev")
        }
    }
}
