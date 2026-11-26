import XCTest

@MainActor
final class IslandContinueTests: XCTestCase {
    private var dir: URL!
    private var asks = 0
    private var sentIDs: [[String]?] = []

    override func setUp() {
        dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        asks = 0
        sentIDs = []
    }

    override func tearDown() { try? FileManager.default.removeItem(at: dir) }

    private func store() -> PanelStore {
        let engine = PanelEngine(
            prep: { _, _ in throw EngineError.notRunning },
            why: { _, _, _ in throw EngineError.notRunning },
            ask: { [unowned self] _, ids, _ in
                asks += 1
                sentIDs.append(ids)
                return PanelAsk(answer: "new", cited: [], cards: [], errorKind: nil)
            })
        return PanelStore(engine: { engine }, meetings: { _ in nil }, folder: { [unowned self] in dir })
    }

    private func card() -> Card {
        Card(id: "c1", path: "docs/a.md", kind: "docs", source: "repo", byline: nil, url: nil, lineStart: 1, lineEnd: 2,
             title: "a", snippet: "x", score: nil, githubUrl: nil, localPath: nil, chunk: 7, repeat: nil)
    }

    func testContinueSeedsPanelWithCardWithoutNewRequest() async {
        let s = store()
        PanelEntries.continueIsland(question: "기본값이 몇이죠?", answer: "30초예요 [1]", cards: [card()], in: s)
        await s.settle()
        XCTAssertTrue(s.isOpen)
        // docs/36 0-1: the island is outside the pages, so every meeting and connected source (the card's own sources too)
        XCTAssertEqual(s.scope, .all)
        XCTAssertEqual(s.current?.origin, .all)
        XCTAssertEqual(s.current?.turns.count, 1)
        let turn = s.current?.turns.first
        XCTAssertEqual(turn?.question, "기본값이 몇이죠?")
        XCTAssertEqual(turn?.body, .ask(PanelAsk(answer: "30초예요 [1]", cited: [], cards: [card()], errorKind: nil)))
        XCTAssertEqual(asks, 0)
        XCTAssertEqual(s.history.first?.id, s.current?.id)
    }

    /// A follow-up after continuing searches everything, not only a meeting.
    func testFollowUpAfterContinuingSearchesEverything() async {
        let s = store()
        s.open(scope: .meeting("mtg"), via: .page)
        PanelEntries.continueIsland(question: "q", answer: "답", cards: [], in: s)
        s.ask("그거 누가 정했어?")
        await s.settle()
        XCTAssertEqual(sentIDs.count, 1)
        XCTAssertNil(sentIDs[0])
    }

    func testLineShowsOnlyForAnAnswerOrCards() {
        XCTAssertTrue(PanelEntries.canContinue(answer: "답", cards: []))
        XCTAssertTrue(PanelEntries.canContinue(answer: nil, cards: [card()]))
        XCTAssertFalse(PanelEntries.canContinue(answer: nil, cards: []))
        XCTAssertFalse(PanelEntries.canContinue(answer: "", cards: []))
    }

    func testContinueWithoutAnythingDoesNothing() {
        let s = store()
        PanelEntries.continueIsland(question: "q", answer: nil, cards: [], in: s)
        XCTAssertFalse(s.isOpen)
        XCTAssertNil(s.current)
    }
}
