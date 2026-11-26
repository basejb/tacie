import XCTest
@testable import Tacie

final class SonioxTokensTests: XCTestCase {
    private func tok(_ text: String, _ start: Int? = nil, _ end: Int? = nil, final: Bool = true) -> SonioxToken {
        SonioxToken(text: text, startMs: start, endMs: end, isFinal: final, confidence: nil)
    }

    func testPcm16ClampsAndScales() {
        let data = SonioxTokens.pcm16([0, 1, -1, 2])
        let values = data.withUnsafeBytes { Array($0.bindMemory(to: Int16.self)) }.map { Int16(littleEndian: $0) }
        XCTAssertEqual(values, [0, 32767, -32767, 32767])
        XCTAssertEqual(data.count, 8)
    }

    func testTextJoinsPiecesAndTrims() {
        XCTAssertEqual(SonioxTokens.text([tok(" 아"), tok(" 네"), tok(".")]), "아 네.")
    }

    func testWordsStartAtASpace() {
        let words = SonioxTokens.words([tok("회", 0, 100), tok("사", 100, 200), tok("가", 200, 300), tok(" 좀", 400, 500)],
                                       offsetMs: 1000)
        XCTAssertEqual(words, [SpokenWord(text: "회사가", start: 1000, end: 1300), SpokenWord(text: "좀", start: 1400, end: 1500)])
    }

    func testPunctuationStaysOnTheWord() {
        let words = SonioxTokens.words([tok(" 맞죠", 0, 300), tok("?", 300, 320)], offsetMs: 0)
        XCTAssertEqual(words, [SpokenWord(text: "맞죠?", start: 0, end: 320)])
    }

    func testTokenDecodesSonioxKeys() throws {
        let t = try JSONDecoder().decode(SonioxToken.self, from: Data(#"{"text":" 네","start_ms":10,"end_ms":20,"is_final":true,"confidence":0.9}"#.utf8))
        XCTAssertEqual(t, SonioxToken(text: " 네", startMs: 10, endMs: 20, isFinal: true, confidence: 0.9))
    }
}
