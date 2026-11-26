import AppKit
import SwiftUI
import XCTest

/// The meeting page's top row (End while live, copy and the menu after) sits in the main window's title bar.
/// On macOS 26 the page's scroll view reaches up under that row; a click on the row must still reach the row.
@MainActor
final class TitleRowPageTests: XCTestCase {
    func testTopRowIsNotUnderTheScrollView() throws {
        let page = TitleRowPage {
            // a button as wide as the row: empty space in a row lets clicks through anyway, a button must not
            Button {} label: { Color.clear.frame(maxWidth: .infinity, maxHeight: .infinity).contentShape(Rectangle()) }
                .buttonStyle(.plain)
                .frame(height: 52)
        } content: {
            Text("본문").frame(maxWidth: .infinity, minHeight: 2000, alignment: .top)
        }
        let w = NSWindow(contentViewController: NSHostingController(rootView: page.ignoresSafeArea()))
        MainWindowShape.apply(w)
        w.setContentSize(NSSize(width: 900, height: 700))
        w.contentView?.layoutSubtreeIfNeeded()
        let frame = try XCTUnwrap(w.contentView?.superview)

        // every point along the row's middle line lands on the row, not in the scroll view
        for x in stride(from: 100.0, through: frame.bounds.width - 20, by: 20) {
            var v = frame.hitTest(NSPoint(x: x, y: frame.bounds.height - 26))
            XCTAssertNotNil(v)
            while let view = v {
                XCTAssertFalse(view is NSScrollView || view is NSClipView, "x \(Int(x)) in the top row lands in the scroll view")
                v = view.superview
            }
        }
        // and the body still scrolls
        var below = frame.hitTest(NSPoint(x: 450, y: frame.bounds.height - 300))
        var inScroll = false
        while let view = below { inScroll = inScroll || view is NSScrollView; below = view.superview }
        XCTAssertTrue(inScroll, "the body below the row is not in the scroll view")
    }

    // MARK: jump buttons

    func testJumpButtonsOnlyOnALongPageAndOnlyWhereTheyGo() {
        let vp: CGFloat = 700
        // a page under two screens: no buttons
        XCTAssertFalse(PageSpot.Shown(content: CGRect(x: 0, y: 0, width: 800, height: 1300), viewport: vp).long)
        // at the top of a long page: down only
        let top = PageSpot.Shown(content: CGRect(x: 0, y: 0, width: 800, height: 5000), viewport: vp)
        XCTAssertEqual([top.long, top.up, top.down], [true, false, true])
        // the middle: both
        let mid = PageSpot.Shown(content: CGRect(x: 0, y: -2000, width: 800, height: 5000), viewport: vp)
        XCTAssertEqual([mid.up, mid.down], [true, true])
        // the bottom: up only
        let end = PageSpot.Shown(content: CGRect(x: 0, y: -4300, width: 800, height: 5000), viewport: vp)
        XCTAssertEqual([end.up, end.down], [true, false])
        // a page not laid out yet shows nothing
        XCTAssertFalse(PageSpot.Shown(content: CGRect(x: 0, y: 0, width: 800, height: 5000), viewport: 0).long)
    }

    /// The buttons over a page, written as a PNG to look at when TACIE_RENDER_DIR is set (a window never shown
    /// draws no SwiftUI, so this draws the page by hand).
    func testRenderJumpButtons() throws {
        guard let dir = ProcessInfo.processInfo.environment["TACIE_RENDER_DIR"] else { throw XCTSkip("no TACIE_RENDER_DIR") }
        for (name, up, down) in [("top", false, true), ("middle", true, true), ("bottom", true, false)] {
            let page = ZStack(alignment: .bottomTrailing) {
                VStack(alignment: .leading, spacing: 14) {
                    ForEach(0..<14, id: \.self) { i in
                        Text("화자 \(i % 3 + 1) · 오후 2:\(10 + i)  이번 배포는 금요일 전에 끝내야 하니까 인증 쪽을 먼저 볼게요.")
                            .uiFont(14.5).foregroundStyle(Theme.ink.opacity(0.88))
                    }
                }
                .padding(.horizontal, 56).padding(.top, 20)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                JumpButtons(up: up, down: down) { _ in }
                    .padding(.trailing, 16).padding(.bottom, 16)
            }
            .frame(width: 720, height: 420)
            .background(Theme.paper)
            let r = ImageRenderer(content: page)
            r.scale = 2
            let image = try XCTUnwrap(r.nsImage)
            let rep = try XCTUnwrap(image.tiffRepresentation.flatMap(NSBitmapImageRep.init(data:)))
            try rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: dir).appendingPathComponent("jump-\(name).png"))
        }
    }
}
