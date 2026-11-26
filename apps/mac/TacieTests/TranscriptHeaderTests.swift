import SwiftUI
import XCTest
@testable import Tacie

/// The Transcript tab's header is measured at the column's width (it was measured unwrapped, and its last line drew
/// over the first turns when the column was narrow).
@MainActor
final class TranscriptHeaderTests: XCTestCase {
    func testHeaderMeasuresWrappedAtNarrowWidth() {
        let long = String(repeating: "전세사기 피해자 지원 3건 법률안 심사 ", count: 6)
        let host = TranscriptHeaderHost(rootView: AnyView(
            VStack(alignment: .leading, spacing: 4) {
                Text(long).font(.system(size: 28, weight: .bold))
                Text("녹음하지 않은 회의예요").font(.system(size: 12))
            }
            .frame(maxWidth: .infinity, alignment: .leading)))
        let wide = host.height(forWidth: 2000)
        let narrow = host.height(forWidth: 300)
        XCTAssertGreaterThan(narrow, wide * 2, "narrow \(narrow) wide \(wide)")
    }

    /// The measuring copy is marked as a probe (so the title field and its click monitor stay out of it), and the
    /// on-screen header is never measured through a second unmarked copy.
    func testMeasuringCopyIsAProbe() {
        let seen = ProbeLog()
        let host = TranscriptHeaderHost(rootView: AnyView(ProbeView(log: seen)))
        seen.values = []
        _ = host.height(forWidth: 300)
        XCTAssertFalse(seen.values.isEmpty)
        XCTAssertTrue(seen.values.allSatisfy { $0 }, "\(seen.values)")
    }

    func testPageLeavesRoomForTheWholeHeader() {
        let page = TranscriptTextView.make()
        page.header.rootView = AnyView(
            VStack(alignment: .leading) {
                Text(String(repeating: "긴 회의 제목 ", count: 20)).font(.system(size: 28, weight: .bold))
                Text("녹음하지 않은 회의예요")
            }
            .frame(maxWidth: .infinity, alignment: .leading))
        page.layoutPage(width: 600)
        let needed = page.header.height(forWidth: page.header.frame.width)
        XCTAssertGreaterThanOrEqual(page.header.frame.height, needed - 0.5)
        XCTAssertGreaterThanOrEqual(page.top, page.header.frame.maxY - 0.5)
    }
}

private final class ProbeLog {
    var values: [Bool] = []
}

private struct ProbeView: View {
    let log: ProbeLog
    @Environment(\.layoutProbe) private var probe

    var body: some View {
        log.values.append(probe)
        return Text("x")
    }
}
