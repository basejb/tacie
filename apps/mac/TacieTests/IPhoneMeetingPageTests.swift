import XCTest
import SwiftUI

/// An iPhone meeting on the Mac (design notes 4-3): no
/// transcript or recording here, so nothing that needs them shows; the list and the delete dialog say iPhone.
@MainActor
final class IPhoneMeetingPageTests: XCTestCase {
    private let start = Date(timeIntervalSince1970: 1_794_000_000)

    private func meeting(iphone: Bool) -> SavedMeeting {
        var m = SavedMeeting(id: iphone ? "iphone-1794000000" : "20261116-100000", mode: iphone ? "inperson" : "remote", start: start)
        m.end = start.addingTimeInterval(1800)
        m.title = iphone ? "주간 회의" : "결제 화면 정리"
        m.notes = .done
        m.summary = [iphone ? "출시 일정을 다음 주 금요일로 미루기로 했어요." : "카드 결제만 먼저 내기로 했어요."]
        m.ledger = [LedgerEntry(kind: "decision", text: "카드 결제만 먼저 내요"),
                    LedgerEntry(kind: "commitment", text: "결제 화면 시안 공유")]
        if iphone { m.source = "iphone" }
        return m
    }

    func test_iphoneMeetingHidesRecordingAndRewrite() {
        let phone = meeting(iphone: true), mac = meeting(iphone: false)
        for a in [IPhoneMeetingUI.Action.listen, .resume, .rewrite, .rename, .deleteRecording] {
            XCTAssertFalse(IPhoneMeetingUI.allows(a, for: phone), "\(a) on an iPhone meeting")
            XCTAssertTrue(IPhoneMeetingUI.allows(a, for: mac), "\(a) on a Mac meeting")
        }
        // just ended: a Mac meeting can be taken up again, an iPhone one cannot (and its header says nothing of it)
        let now = start.addingTimeInterval(1800 + 60)
        XCTAssertFalse(phone.resumable(now: now))
        XCTAssertTrue(mac.resumable(now: now))
        XCTAssertFalse(phone.pageHeader(calendar: MainView.calendar, now: now).contains("전에 끝났어요"))
        // the transcript tab is there, with one sentence in it; the row says where the meeting came from
        XCTAssertEqual(IPhoneMeetingUI.transcriptNote(for: phone), "대화 기록은 iPhone에 있어요")
        XCTAssertNil(IPhoneMeetingUI.transcriptNote(for: mac))
        XCTAssertEqual(IPhoneMeetingUI.rowTag(for: phone), "iPhone")
        XCTAssertNil(IPhoneMeetingUI.rowTag(for: mac))
    }

    /// 설정 → 계정 → 기기: only with iPhone meetings here; each row counts its own, the iPhone's says when they came.
    func test_devicesSection() {
        let phone = meeting(iphone: true), mac = meeting(iphone: false)
        XCTAssertFalse(IPhoneMeetingUI.showsDevices([mac]))
        XCTAssertTrue(IPhoneMeetingUI.showsDevices([mac, phone]))
        XCTAssertEqual(IPhoneMeetingUI.macLine([mac, phone, mac]), "회의 2개")
        XCTAssertEqual(IPhoneMeetingUI.iphoneLine([mac, phone], pulledAt: nil), "iPhone 회의 1개")
        let now = start.addingTimeInterval(3600)
        let today = IPhoneMeetingUI.iphoneLine([phone], pulledAt: start, now: now, calendar: MainView.calendar)
        XCTAssertTrue(today.hasPrefix("iPhone 회의 1개 · ") && today.hasSuffix("에 받아 왔어요"), today)
        XCTAssertFalse(today.contains("2026"), "today: the time only, \(today)")
        let older = IPhoneMeetingUI.iphoneLine([phone], pulledAt: start.addingTimeInterval(-3 * 86400), now: now, calendar: MainView.calendar)
        XCTAssertNotEqual(older, today, "another day carries its date")
    }

    func test_iphoneMeetingDeleteSaysIPhone() {
        XCTAssertEqual(IPhoneMeetingUI.deleteMessage(for: meeting(iphone: true)), "iPhone의 녹음과 대화 기록도 함께 지워져요")
        XCTAssertEqual(IPhoneMeetingUI.deleteMessage(for: meeting(iphone: false)), "받아쓴 대화와 정리가 이 Mac에서 사라져요.")
    }

    /// 개인정보 → 모두 지우기 names how many iPhone meetings go on the iPhone too; nothing when there are none.
    func test_removeAllNoteCountsIPhoneMeetings() {
        var other = SavedMeeting(id: "iphone-1794003600", mode: "inperson", start: start)
        other.source = "iphone"
        let phone = meeting(iphone: true), mac = meeting(iphone: false)
        XCTAssertEqual(IPhoneMeetingUI.iphoneCount([phone, mac, other]), 2)
        XCTAssertEqual(IPhoneMeetingUI.removeAllNote(for: [phone, mac, other]),
                       "iPhone에서 기록한 회의 2개는 iPhone의 녹음과 대화 기록까지 함께 지워져요.")
        XCTAssertNil(IPhoneMeetingUI.removeAllNote(for: [mac]))
        XCTAssertNil(IPhoneMeetingUI.removeAllNote(for: []))
        // the dialog's message: as before with only this Mac's meetings, two lines with iPhone ones
        XCTAssertEqual(IPhoneMeetingUI.removeAllMessage(for: [mac]),
                       "이 Mac에 저장한 회의 1개의 대화 기록과 회의 정리가 사라져요. 되돌릴 수 없어요.")
        XCTAssertEqual(IPhoneMeetingUI.removeAllMessage(for: [phone, mac, other]),
                       "이 Mac의 회의 기록 3개가 모두 지워져요. 되돌릴 수 없어요.\niPhone에서 기록한 회의 2개는 iPhone의 녹음과 대화 기록까지 함께 지워져요.")
    }

    // MARK: renders

    /// The list rows, the page's top and the transcript tab, light and dark, when TACIE_IPHONE_MEETING_RENDER_DIR names
    /// a folder (pass it as TEST_RUNNER_TACIE_IPHONE_MEETING_RENDER_DIR); nothing opens on screen. The meeting page itself
    /// is not part of the tests: its top is drawn here from the same parts, laid out as the page lays them out.
    func testRender() throws {
        guard let path = ProcessInfo.processInfo.environment["TACIE_IPHONE_MEETING_RENDER_DIR"] else { throw XCTSkip("no render folder") }
        let out = URL(fileURLWithPath: path)
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        let phone = meeting(iphone: true), mac = meeting(iphone: false)
        let now = start.addingTimeInterval(1800 + 120)
        for scheme in [ColorScheme.light, .dark] {
            let name = scheme == .light ? "light" : "dark"
            png(Rows(meetings: [phone, mac], now: now), width: 300, scheme, "rows-\(name)", to: out)
            png(PageTop(meeting: phone, now: now, transcript: false), width: 720, scheme, "page-top-\(name)", to: out)
            png(PageTop(meeting: phone, now: now, transcript: true), width: 720, scheme, "transcript-\(name)", to: out)
        }
    }

    private struct Rows: View {
        let meetings: [SavedMeeting]
        let now: Date
        @Namespace private var ns
        var body: some View {
            VStack(spacing: 2) {
                ForEach(meetings, id: \.id) { MeetingRow(meeting: $0, live: false, now: now, selected: $0.id == meetings.first?.id, ns: ns) }
                ForEach(meetings, id: \.id) { MeetingRow(meeting: $0, live: false, now: now, selected: false, ns: ns, full: false) }
            }
            .padding(8)
        }
    }

    /// MeetingPage's top row, header and tabs for an ended meeting with no recording (so no player or 이어 가기 below).
    private struct PageTop: View {
        let meeting: SavedMeeting
        let now: Date
        let transcript: Bool
        var body: some View {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 2) {
                    Spacer()
                    IconButton(symbol: "doc.on.doc")
                    IconButton(symbol: "square.and.arrow.up")
                    IconButton(symbol: "ellipsis")
                }
                .padding(.horizontal, 14).frame(height: 52)
                VStack(alignment: .leading, spacing: 0) {
                    HStack(spacing: 8) {
                        Text(meeting.pageHeader(calendar: MainView.calendar, now: now)).uiFont(12.5).foregroundStyle(Theme.ink3)
                        AITag()
                    }
                    .padding(.bottom, 10)
                    EditableTitle(text: meeting.displayTitle(calendar: MainView.calendar), editing: .constant(false)) { _ in }
                        .padding(.bottom, 30)
                    HStack(spacing: 22) {
                        tab(String(localized: "정리"), on: !transcript)
                        tab(String(localized: "대화 기록"), on: transcript)
                        Spacer(minLength: 0)
                    }
                    .overlay(alignment: .bottom) { Rectangle().fill(Theme.ink.opacity(0.08)).frame(height: 1) }
                    .padding(.bottom, 32)
                    if transcript, let note = IPhoneMeetingUI.transcriptNote(for: meeting) {
                        TranscriptElsewhere(text: note)
                    } else {
                        DocHeading(title: String(localized: "요약"))
                        Text(meeting.summary.joined()).uiFont(15).foregroundStyle(Theme.ink)
                    }
                }
                .padding(.horizontal, 56).padding(.top, 14).padding(.bottom, 24)
            }
        }

        private func tab(_ title: String, on: Bool) -> some View {
            Text(title).uiFont(14.5, weight: .semibold).foregroundStyle(on ? Theme.ink : Theme.ink2)
                .padding(.bottom, 10)
                .overlay(alignment: .bottom) { if on { Rectangle().fill(Theme.ink).frame(height: 2) } }
        }
    }

    private func png<V: View>(_ view: V, width: CGFloat, _ scheme: ColorScheme, _ name: String, to out: URL) {
        AppearanceMode.isDark = scheme == .dark
        defer { AppearanceMode.isDark = false }
        let framed = view.frame(width: width, alignment: .leading).padding(.vertical, 12).background(Theme.paper)
            .environment(\.colorScheme, scheme)
        let r = ImageRenderer(content: framed)
        r.scale = 2
        guard let tiff = r.nsImage?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff),
              let data = rep.representation(using: .png, properties: [:]) else { return XCTFail("no image for \(name)") }
        try? data.write(to: out.appendingPathComponent("\(name).png"))
    }
}
