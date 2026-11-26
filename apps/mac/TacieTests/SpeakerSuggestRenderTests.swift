import XCTest
import SwiftUI
@testable import Tacie

/// Draws the people strip with speaker suggestions and the namer to PNG for a look. Runs only when
/// TACIE_SUGGEST_RENDER_DIR names a folder (pass it as TEST_RUNNER_TACIE_SUGGEST_RENDER_DIR); nothing opens on screen.
@MainActor
final class SpeakerSuggestRenderTests: XCTestCase {
    private let base = Date(timeIntervalSince1970: 1_792_000_000)

    private func png<V: View>(_ view: V, _ name: String, scheme: ColorScheme, to out: URL) {
        let framed = view.padding(28).frame(width: 760, alignment: .topLeading).background(Theme.paper).environment(\.colorScheme, scheme)
        let r = ImageRenderer(content: framed)
        r.scale = 2
        guard let tiff = r.nsImage?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
              let data = rep.representation(using: .png, properties: [:]) else { return XCTFail("no image for \(name)") }
        try? data.write(to: out.appendingPathComponent("\(name).png"))
    }

    func testRenderStates() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_SUGGEST_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        var m = SavedMeeting(mode: "inperson", start: base)
        let said: [(String, String)] = [("화자 1", "지난주 결제 실패는 대부분 카드사 쪽 일시 오류였어요."), ("화자 3", "바로 하면 결제사 호출 한도에 걸려요. 그래서 예전에 간격을 둔 거예요."),
                                     ("화자 2", "알림만 앱 푸시로 더 보내는 게 어때요?"), ("화자 1", "좋아요."), ("화자 3", "한도는 문서로 남길게요.")]
        m.transcript = said.enumerated().map { i, x in
            TranscriptLine(key: "e0:s1u\(i)", at: base + Double(i), speaker: nil, text: x.1, question: false, voice: x.0, name: nil)
        }
        m.voices = [VoiceEntry(key: "화자 1", name: "민호", person: "p0")]
        let people = [Person(id: "p0", name: "민호", vector: [], seconds: 30, meetings: 3, last: base, color: 0),
                      Person(id: "p1", name: "지윤", vector: [], seconds: 30, meetings: 2, last: base - 4 * 86400, color: 4),
                      Person(id: "p2", name: "서연", vector: [], seconds: 30, meetings: 1, last: base - 7 * 86400, color: 3),
                      Person(id: "p3", name: "해리", vector: [], seconds: 30, meetings: 1, last: base - 11 * 86400, color: 0)]
        var editing = SpeakerEditing(apply: { _, _, _ in }, renamePerson: { _, _ in }, forgetPerson: { _ in })
        editing.suggestions = ["화자 3": people[1], "화자 2": people[2]]
        for scheme in [ColorScheme.light, .dark] {
            let tag = scheme == .light ? "light" : "dark"
            png(PeopleStrip(meeting: m, people: people, editing: editing).frame(height: 80, alignment: .top), "1-strip-\(tag)", scheme: scheme, to: out)
            png(PeopleStrip(meeting: m, people: people, editing: editing, peek: "화자 3").frame(height: 80, alignment: .top), "2-card-\(tag)", scheme: scheme, to: out)
            png(Namer(meeting: m, target: NamerTarget(key: "화자 3"), people: people, editing: editing) {}, "3-namer-\(tag)", scheme: scheme, to: out)
            // a meeting linked to a calendar event: its invited people between the speakers and the remembered people
            var linked = m
            linked.link(MeetingEvent(id: "ev", title: "결제 재시도 회의", start: base, end: base + 3600),
                        invited: [MeetingInvitee(name: "정하늘", email: nil), MeetingInvitee(name: "윤가람", email: nil),
                                  MeetingInvitee(name: "지윤", email: nil)], meaningful: true)
            png(Namer(meeting: linked, target: NamerTarget(key: "화자 2"), people: people, editing: SpeakerEditing(apply: { _, _, _ in }, renamePerson: { _, _ in }, forgetPerson: { _ in })) {},
                "4-namer-invited-\(tag)", scheme: scheme, to: out)
        }
    }
}
