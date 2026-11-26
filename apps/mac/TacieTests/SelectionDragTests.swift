import AppKit
import SwiftUI
import XCTest
@testable import Tacie

/// A drag over the meeting page's text: it starts beside the letters too (#25) and scrolls the page near its top and
/// bottom while the pointer is still in the window (#24).
@MainActor
final class SelectionDragTests: XCTestCase {
    // MARK: scrolling at the edge

    func testEdgeStepScrollsOnlyNearTheEdges() {
        // the page shows from y 0 to y 600 in the window
        XCTAssertEqual(TextSelectionGroup.edgeStep(y: 300, top: 600, bottom: 0), 0)
        XCTAssertEqual(TextSelectionGroup.edgeStep(y: 551, top: 600, bottom: 0), 0)
        XCTAssertGreaterThan(TextSelectionGroup.edgeStep(y: 590, top: 600, bottom: 0), 0)
        XCTAssertLessThan(TextSelectionGroup.edgeStep(y: 10, top: 600, bottom: 0), 0)
        // faster the further in, up to a cap past the edge
        let near = TextSelectionGroup.edgeStep(y: 560, top: 600, bottom: 0)
        let edge = TextSelectionGroup.edgeStep(y: 600, top: 600, bottom: 0)
        let past = TextSelectionGroup.edgeStep(y: 900, top: 600, bottom: 0)
        XCTAssertLessThan(near, edge)
        XCTAssertLessThanOrEqual(edge, past)
        XCTAssertEqual(past, 48)
        XCTAssertEqual(TextSelectionGroup.edgeStep(y: -900, top: 600, bottom: 0), -48)
    }

    func testDragNearTheTopScrollsUnderTheTopRow() throws {
        let (w, group, _) = try page()
        let scroll = try XCTUnwrap(group.shown(in: w)?.enclosingScrollView)
        let clip = scroll.contentView
        clip.scroll(to: NSPoint(x: 0, y: 1500)); scroll.reflectScrolledClipView(clip)
        let start = clip.bounds.origin.y
        let rowBottom = clip.convert(clip.bounds, to: nil).maxY - scroll.contentInsets.top

        // in the middle of the page: no scroll
        group.scrollAtEdge(NSPoint(x: 400, y: rowBottom - 300), in: w)
        XCTAssertEqual(clip.bounds.origin.y, start)
        // just under the top row, the pointer still in the window: scrolls up
        group.scrollAtEdge(NSPoint(x: 400, y: rowBottom - 10), in: w)
        XCTAssertLessThan(clip.bounds.origin.y, start)
        // near the bottom of the window: scrolls down
        let up = clip.bounds.origin.y
        group.scrollAtEdge(NSPoint(x: 400, y: 10), in: w)
        XCTAssertGreaterThan(clip.bounds.origin.y, up)
        // at the very top of the page it stops
        clip.scroll(to: NSPoint(x: 0, y: -scroll.contentInsets.top)); scroll.reflectScrolledClipView(clip)
        group.scrollAtEdge(NSPoint(x: 400, y: rowBottom + 20), in: w)
        XCTAssertEqual(clip.bounds.origin.y, -scroll.contentInsets.top)
    }

    // MARK: starting beside the letters

    func testDragFromBesideShortLinesSelectsThem() throws {
        let (w, group, pad) = try page()
        let texts = shownTexts(in: w)
        XCTAssertGreaterThan(texts.count, 3)
        let (a, b) = (texts[1], texts[3])
        let fa = a.convert(a.bounds, to: nil), fb = b.convert(b.bounds, to: nil)
        // far right of a short line, on nothing but the page
        let from = NSPoint(x: fa.maxX + 300, y: fa.midY)
        let to = NSPoint(x: fb.maxX + 300, y: fb.midY)
        XCTAssertNotNil(pad.handle(event(.leftMouseDown, at: from, in: w)))
        // a press alone selects nothing
        XCTAssertFalse(group.hasSelection)
        XCTAssertNil(pad.handle(event(.leftMouseDragged, at: NSPoint(x: from.x, y: from.y - 6), in: w)))
        XCTAssertNil(pad.handle(event(.leftMouseDragged, at: to, in: w)))
        XCTAssertNotNil(pad.handle(event(.leftMouseUp, at: to, in: w)))
        // from the end of the first line to the end of the third: the second and the third
        XCTAssertEqual(group.selectedText, [texts[2].string, b.string].joined(separator: "\n"))
    }

    func testPressOnTheLettersIsLeftToTheParagraph() throws {
        let (w, group, pad) = try page()
        let t = shownTexts(in: w)[1]
        let f = t.convert(t.bounds, to: nil)
        let on = NSPoint(x: f.midX, y: f.midY)
        _ = pad.handle(event(.leftMouseDown, at: on, in: w))
        // the paragraph runs its own drag; the column lets these pass
        XCTAssertNotNil(pad.handle(event(.leftMouseDragged, at: NSPoint(x: on.x, y: on.y - 40), in: w)))
        XCTAssertFalse(group.hasSelection)
    }

    func testAClickWithoutMovingPassesThrough() throws {
        let (w, group, pad) = try page()
        let t = shownTexts(in: w)[1]
        let f = t.convert(t.bounds, to: nil)
        let p = NSPoint(x: f.maxX + 200, y: f.midY)
        XCTAssertNotNil(pad.handle(event(.leftMouseDown, at: p, in: w)))
        // a tremble under 3 points is still a click
        XCTAssertNotNil(pad.handle(event(.leftMouseDragged, at: NSPoint(x: p.x + 1, y: p.y + 1), in: w)))
        XCTAssertNotNil(pad.handle(event(.leftMouseUp, at: p, in: w)))
        XCTAssertFalse(group.hasSelection)
    }

    // MARK: helpers

    private func page() throws -> (NSWindow, TextSelectionGroup, SelectionPadView) {
        let group = TextSelectionGroup()
        let page = TitleRowPage {
            Color.clear.frame(height: 52)
        } content: {
            VStack(alignment: .leading, spacing: 0) {
                ForEach(0..<60, id: \.self) { i in
                    HStack(alignment: .top, spacing: 12) {
                        Circle().fill(.red).frame(width: 26, height: 26)
                        VStack(alignment: .leading, spacing: 4) {
                            Text("화자 1").frame(height: 22)
                            SelectableText(id: "p\(i)", text: "짧은 줄 \(i)", group: group)
                        }
                        Spacer(minLength: 0)
                    }
                    .padding(.vertical, 10)
                    // as a turn does: the whole row answers the pointer
                    .contentShape(Rectangle())
                    .onHover { _ in }
                }
            }
            .padding(.horizontal, 56)
            .background { SelectionPad(group: group) }
            .frame(maxWidth: 792, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
        let w = NSWindow(contentViewController: NSHostingController(rootView: page.ignoresSafeArea()))
        MainWindowShape.apply(w)
        w.setContentSize(NSSize(width: 900, height: 700))
        w.contentView?.layoutSubtreeIfNeeded()
        let root = try XCTUnwrap(w.contentView)
        let pad = try XCTUnwrap(find(SelectionPadView.self, in: root))
        let scroll = try XCTUnwrap(pad.enclosingScrollView)
        // from the top, so the first turns are the ones shown
        scroll.contentView.scroll(to: NSPoint(x: 0, y: -scroll.contentInsets.top))
        scroll.reflectScrolledClipView(scroll.contentView)
        w.contentView?.layoutSubtreeIfNeeded()
        addTeardownBlock { @MainActor in group.clear() }
        return (w, group, pad)
    }

    /// The paragraphs fully in view under the top row, top to bottom.
    private func shownTexts(in w: NSWindow) -> [SelectableTextView] {
        var out: [SelectableTextView] = []
        func walk(_ v: NSView) { if let t = v as? SelectableTextView { out.append(t) }; v.subviews.forEach(walk) }
        if let root = w.contentView { walk(root) }
        return out
            .filter { t in
                guard t.window === w, let s = t.enclosingScrollView else { return false }
                let f = t.convert(t.bounds, to: nil)
                let clip = s.contentView.convert(s.contentView.bounds, to: nil)
                return f.minY > clip.minY + 60 && f.maxY < clip.maxY - s.contentInsets.top - 60
            }
            .sorted { $0.convert($0.bounds, to: nil).minY > $1.convert($1.bounds, to: nil).minY }
    }

    private func event(_ type: NSEvent.EventType, at p: NSPoint, in w: NSWindow) -> NSEvent {
        NSEvent.mouseEvent(with: type, location: p, modifierFlags: [], timestamp: 0, windowNumber: w.windowNumber,
                           context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
    }

    private func find<T: NSView>(_ t: T.Type, in v: NSView) -> T? {
        if let v = v as? T { return v }
        for s in v.subviews { if let f = find(t, in: s) { return f } }
        return nil
    }
}
