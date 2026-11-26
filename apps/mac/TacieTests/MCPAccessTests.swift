import XCTest

final class MCPAccessTests: XCTestCase {
    private func defaults() -> UserDefaults {
        let name = "mcp-access-\(UUID().uuidString)"
        let d = UserDefaults(suiteName: name)!
        d.removePersistentDomain(forName: name)
        return d
    }

    func testOnIsPerAccount() {
        let d = defaults()
        let a = AccountSpace(id: "aaa"), b = AccountSpace(id: "bbb")
        XCTAssertFalse(MCPAccess.isOn(d, space: a))
        MCPAccess.set(true, d, space: a)
        XCTAssertTrue(MCPAccess.isOn(d, space: a))
        XCTAssertFalse(MCPAccess.isOn(d, space: b))
        XCTAssertFalse(MCPAccess.isOn(d, space: .signedOut))
        MCPAccess.set(false, d, space: a)
        XCTAssertFalse(MCPAccess.isOn(d, space: a))
    }

    func testServerAllowsMatrix() {
        let d = defaults()
        XCTAssertTrue(MCPAccess.serverAllows(d))  // an older server sends none
        for (value, allowed) in [("all", true), ("off", false), ("paid", false), ("junk", true)] {
            MCPAccess.remember(value, defaults: d)
            XCTAssertEqual(MCPAccess.serverAllows(d), allowed, value)
        }
        MCPAccess.remember(nil, defaults: d)  // keeps what it had
        XCTAssertTrue(MCPAccess.serverAllows(d))
    }

    func testOpenNeedsOfferedOnAndServer() {
        let d = defaults()
        let s = AccountSpace(id: "aaa")
        XCTAssertFalse(MCPAccess.open(d, space: s, offered: true))
        MCPAccess.set(true, d, space: s)
        XCTAssertTrue(MCPAccess.open(d, space: s, offered: true))
        XCTAssertFalse(MCPAccess.open(d, space: s, offered: false))
        MCPAccess.remember("paid", defaults: d)
        XCTAssertFalse(MCPAccess.open(d, space: s, offered: true))
    }

    func testFlagIsAdoptedWithTheAccount() {
        XCTAssertTrue(SpaceAdoption.defaultsKeys.contains(MCPAccess.onKey))
    }
}
