import AppKit
import SwiftUI

/// The Transcript tab: the whole transcript as one native text view, so a drag, the wheel during a drag, scrolling at
/// the edge, ⌘A and ⌘C all work as in any Mac text. A page of hundreds of separate paragraphs had to make each of these
/// itself (#24, #25) and was slow to open on a long meeting (#26).
///
/// Each turn reads: the speaker's dot in the left margin, the name (a click opens the namer) and the time, the words
/// (with the recording, a click on a word plays from there), and what Tacie found for its questions. An unheard stretch
/// sits between turns as a grey line that plays from its start.

/// The sentence being said in a meeting going on (docs/25): Soniox's final words so far and the partial ones after
/// them. It ends the transcript until the engine turns it into a line of its own.
struct LiveLine: Equatable {
    /// Sentences Soniox finished whose lines have not come back from the engine yet: they stay until then.
    var held: [String] = []
    let said: String
    let partial: String
    var isEmpty: Bool { held.isEmpty && said.isEmpty && partial.trimmingCharacters(in: .whitespaces).isEmpty }
    /// The final part as one text: the held sentences, then what is final of the one being said.
    var final: String { (held + [said]).filter { !$0.isEmpty }.joined(separator: " ") }
}

/// The transcript laid out as text, with where each turn, word and unheard stretch sits in it.
struct TranscriptDoc {
    /// Space under a question's turn: its fill reaches 10 below the text and the next one's 10 above (`box`), so two
    /// questions in a row keep 8 between their fills.
    static let boxedGap: CGFloat = 28
    struct Turn {
        let range: NSRange
        let header: NSRange?
        let name: NSRange?
        let tag: NSRange?
        let nameText: String?
        let key: String?
        let color: Int
        let question: Bool
        let target: NamerTarget?
    }

    struct Word {
        let range: NSRange
        let ms: Int?
        let line: String
        let index: Int
    }

    struct Unheard {
        let range: NSRange
        let ms: Int
    }

    let text: NSAttributedString
    let turns: [Turn]
    let words: [Word]
    let unheard: [Unheard]
    let speakers: Bool
    /// The left margin of the words: the dot's column with speakers, the time's without.
    let indent: CGFloat
    /// What the looks depend on besides the text (names, colours): a change here redraws all of it.
    let looks: [String]

    static func make(meeting m: SavedMeeting, people: [Person], calendar: Calendar, unheard spans: [AudioSpan],
                     scale: CGFloat, live: LiveLine? = nil, translations: Bool = true) -> TranscriptDoc {
        let speakers = m.hasSpeakers
        let indent = (speakers ? 38 : 70) * scale
        let out = NSMutableAttributedString()
        var turns: [Turn] = []
        var words: [Word] = []
        var unheard: [Unheard] = []
        var looks: [String] = []
        var lookMemo: [String: (key: String?, color: Int)] = [:]

        let ink = NSColor(Theme.ink)
        let bodyFont = NSFont.systemFont(ofSize: 14.5 * scale)
        let small = NSFont.systemFont(ofSize: 11.5 * scale)
        let smallBold = NSFont.systemFont(ofSize: 11.5 * scale, weight: .semibold)
        let digits = NSFont.monospacedDigitSystemFont(ofSize: 11.5 * scale, weight: .regular)

        func style(first: CGFloat = indent, before: CGFloat = 0, after: CGFloat = 0, spacing: CGFloat = 3.5,
                   line: CGFloat? = nil) -> NSParagraphStyle {
            let s = NSMutableParagraphStyle()
            s.firstLineHeadIndent = first
            s.headIndent = indent
            s.paragraphSpacingBefore = before
            s.paragraphSpacing = after
            s.lineSpacing = spacing
            s.lineBreakStrategy = .hangulWordPriority
            if let line { s.minimumLineHeight = line; s.maximumLineHeight = line }
            if !speakers { s.tabStops = [NSTextTab(textAlignment: .left, location: indent)] }
            return s
        }
        func append(_ s: String, _ attrs: [NSAttributedString.Key: Any]) -> NSRange {
            let r = NSRange(location: out.length, length: (s as NSString).length)
            out.append(NSAttributedString(string: s, attributes: attrs))
            return r
        }
        // one formatter for the whole text: a new one per line (SavedMeeting.format) took 0.15 s for 2,400 lines
        let clock = DateFormatter()
        clock.locale = AppLocale.locale
        clock.calendar = calendar
        clock.timeZone = calendar.timeZone
        clock.dateFormat = SavedMeeting.localized("a h:mm")
        func time(_ at: Date) -> String { clock.string(from: at) }

        /// The words of some lines as one paragraph, each word noted with where it plays from.
        // whether the text ends with a turn's words (the sentence being said continues them), not an answer or a gap
        var endsWithWords = false
        func body(_ lines: [TranscriptLine], _ para: NSParagraphStyle) {
            defer { endsWithWords = true }
            // one run per paragraph, not one per word
            var str = ""
            var at = out.length
            for l in lines {
                for (i, w) in l.shownWords.enumerated() {
                    if !str.isEmpty { str += " "; at += 1 }
                    let n = w.text.utf16.count
                    words.append(Word(range: NSRange(location: at, length: n), ms: w.ms, line: l.key, index: i))
                    str += w.text
                    at += n
                }
            }
            _ = append(str, [.font: bodyFont, .foregroundColor: ink.withAlphaComponent(0.88), .paragraphStyle: para])
        }
        /// "↳ Tacie의 답 · …" under a question.
        func found(_ answer: String, last: Bool) {
            endsWithWords = false
            let para = style(before: 4 * scale, after: last ? Self.boxedGap * scale : 0, spacing: 0)
            let grey = ink.withAlphaComponent(0.5)
            _ = append("↳ ", [.font: smallBold, .foregroundColor: grey, .paragraphStyle: para])
            _ = append(String(localized: "transcript.answer", defaultValue: "Tacie의 답"), [.font: smallBold, .foregroundColor: grey, .paragraphStyle: para])
            _ = append(" · ", [.font: small, .foregroundColor: ink.withAlphaComponent(0.3), .paragraphStyle: para])
            _ = append(SavedMeeting.answerStart(answer) + "\n", [.font: small, .foregroundColor: grey, .paragraphStyle: para])
        }
        /// The words' translation in the app's language, under them (docs/30): grey, a little smaller, never instead of them.
        func translated(_ lines: [TranscriptLine], _ para: NSParagraphStyle) -> Bool {
            guard translations else { return false }
            let text = lines.compactMap(\.translation).filter { !$0.isEmpty }.joined(separator: " ")
            guard !text.isEmpty else { return false }
            endsWithWords = false
            _ = append(text, [.font: NSFont.systemFont(ofSize: 13.5 * scale), .foregroundColor: ink.withAlphaComponent(0.5),
                              .paragraphStyle: para])
            return true
        }
        func unheardLine(_ s: AudioSpan) {
            endsWithWords = false
            let para = style(after: 20 * scale)
            let r = append(String(localized: "받아쓰지 못한 부분 · \(RecordingPlayer.timeLabel(s.end - s.start))") + "\n",
                           [.font: bodyFont, .foregroundColor: ink.withAlphaComponent(0.5), .paragraphStyle: para])
            unheard.append(Unheard(range: NSRange(location: r.location, length: r.length - 1), ms: s.start))
        }

        /// "오후 2:49에 멈췄다가 오후 2:51에 다시 들었어요" where the meeting was taken up again (이어서 듣기, docs/35 5-4),
        /// before the first words heard after it.
        var gaps = (m.resumes ?? []).sorted { $0.resumed < $1.resumed }
        func gapLines(before at: Date) {
            while let g = gaps.first, g.resumed <= at {
                gaps.removeFirst()
                endsWithWords = false
                let para = style(first: 0, before: 4 * scale, after: 20 * scale)
                let p = para.mutableCopy() as! NSMutableParagraphStyle
                p.headIndent = 0
                p.alignment = .center
                let said = time(g.stopped) == time(g.resumed)
                    ? String(localized: "\(time(g.resumed))에 잠깐 멈췄다가 다시 이어 갔어요")
                    : String(localized: "\(time(g.stopped))에 멈췄다가 \(time(g.resumed))에 다시 이어 갔어요")
                _ = append(said + "\n",
                           [.font: smallBold, .foregroundColor: ink.withAlphaComponent(0.5), .paragraphStyle: p])
            }
        }

        if speakers {
            for piece in m.transcriptPieces(unheard: spans) {
                switch piece {
                case .unheard(let s): unheardLine(s)
                case .turn(let t):
                    gapLines(before: t.at)
                    let start = out.length
                    // one look per speaker, not per turn: working it out reads the meeting's attendees
                    let lookKey = "\(t.lines[0].voice ?? "")|\(t.lines[0].name ?? "")|\(t.key ?? "")"
                    let look = lookMemo[lookKey] ?? m.turnLook(t, people: people)
                    lookMemo[lookKey] = look
                    var name: NSRange?, tag: NSRange?
                    let head = style(after: 4 * scale, spacing: 0, line: 22 * scale)
                    if let n = t.name {
                        name = append(n, [.font: NSFont.systemFont(ofSize: 13 * scale, weight: .semibold),
                                          .foregroundColor: n == look.key ? ink.withAlphaComponent(0.66) : ink, .paragraphStyle: head])
                        // room for the ⌄ the pointer shows (TranscriptTextView.chevronWidth), then the time
                        _ = append("\u{2002}\u{2002}\u{2002}", [.font: NSFont.systemFont(ofSize: 13 * scale), .paragraphStyle: head])
                    }
                    _ = append(time(t.at), [.font: digits, .foregroundColor: ink.withAlphaComponent(0.5), .paragraphStyle: head])
                    // the language it was said in, beside the time, when a translation is shown under it
                    if translations, let from = t.lines.first(where: { $0.translation != nil })?.translatedFrom {
                        _ = append("\u{2003}", [.font: small, .paragraphStyle: head])
                        _ = append(from.uppercased(), [.font: NSFont.systemFont(ofSize: 10.5 * scale, weight: .semibold),
                                                       .foregroundColor: ink.withAlphaComponent(0.5), .paragraphStyle: head])
                    }
                    if t.question {
                        _ = append("\u{2003}", [.font: small, .paragraphStyle: head])
                        tag = append(String(localized: "질문"), [.font: NSFont.systemFont(ofSize: 10.5 * scale, weight: .semibold),
                                              .foregroundColor: ink.withAlphaComponent(0.66), .paragraphStyle: head])
                    }
                    let header = NSRange(location: start, length: out.length - start)
                    _ = append("\n", [.font: digits, .paragraphStyle: head])
                    let answers = t.lines.compactMap { m.foundAnswer(for: $0) }
                    let gap = answers.isEmpty ? (t.question ? Self.boxedGap : 20) * scale : 0
                    let hasTranslation = translations && t.lines.contains { $0.translation?.isEmpty == false }
                    body(t.lines, style(after: hasTranslation ? 0 : gap))
                    _ = append("\n", [.font: bodyFont, .paragraphStyle: style(after: hasTranslation ? 0 : gap)])
                    if translated(t.lines, style(before: 3 * scale, after: gap, spacing: 2.5)) {
                        _ = append("\n", [.font: bodyFont, .paragraphStyle: style(before: 3 * scale, after: gap, spacing: 2.5)])
                    }
                    for (i, a) in answers.enumerated() { found(a, last: i == answers.count - 1) }
                    let target = m.namerTarget(turn: t)
                    turns.append(Turn(range: NSRange(location: start, length: out.length - start), header: header, name: name,
                                      tag: tag, nameText: t.name, key: look.key, color: look.color, question: t.question,
                                      target: target))
                    looks.append("\(t.name ?? "")|\(look.key ?? "")|\(look.color)")
                }
            }
        } else {
            // every line has no speaker: the time in a column, the words after it
            for row in TranscriptRow.rows(lines: m.transcript, unheard: spans) {
                switch row {
                case .unheard(let s): unheardLine(s)
                case .line(let l):
                    gapLines(before: l.at)
                    let start = out.length
                    let answer = m.foundAnswer(for: l)
                    let para = style(first: 0, after: answer == nil ? (l.question ? Self.boxedGap : 16) * scale : 0)
                    _ = append(time(l.at) + "\t", [.font: digits, .foregroundColor: ink.withAlphaComponent(0.3), .paragraphStyle: para])
                    body([l], para)
                    var tag: NSRange?
                    if l.question {
                        _ = append("\u{2003}", [.font: small, .paragraphStyle: para])
                        tag = append(String(localized: "질문"), [.font: NSFont.systemFont(ofSize: 10.5 * scale, weight: .semibold),
                                              .foregroundColor: ink.withAlphaComponent(0.66), .paragraphStyle: para])
                    }
                    _ = append("\n", [.font: bodyFont, .paragraphStyle: para])
                    let under = style(before: 3 * scale, after: para.paragraphSpacing, spacing: 2.5)
                    if translated([l], under) { _ = append("\n", [.font: bodyFont, .paragraphStyle: under]) }
                    if let answer { found(answer, last: true) }
                    turns.append(Turn(range: NSRange(location: start, length: out.length - start), header: nil, name: nil, tag: tag,
                                      nameText: nil, key: nil, color: 0, question: l.question, target: nil))
                }
            }
        }
        // no empty line under the last turn
        if out.length > 0, out.string.hasSuffix("\n") { out.deleteCharacters(in: NSRange(location: out.length - 1, length: 1)) }
        // the sentence being said: under the last turn, without a name yet (the engine finds the speaker when it ends);
        // the words not final yet are light
        // after the last turn's words, in their paragraph: when its line comes back the words stay where they are
        // (only darker); a new speaker's line moves them under a new name
        if let live, !live.isEmpty {
            let final = live.final
            let partial = final.isEmpty ? String(live.partial.drop { $0 == " " }) : live.partial
            let para: NSParagraphStyle
            if endsWithWords, out.length > 0,
               let last = out.attribute(.paragraphStyle, at: out.length - 1, effectiveRange: nil) as? NSParagraphStyle {
                para = last
                if !final.isEmpty || !partial.hasPrefix(" ") { _ = append(" ", [.font: bodyFont, .paragraphStyle: para]) }
            } else {
                para = style(before: out.length > 0 ? 20 * scale : 0)
                if out.length > 0 { _ = append("\n", [.font: bodyFont, .paragraphStyle: para]) }
            }
            _ = append(final, [.font: bodyFont, .foregroundColor: ink.withAlphaComponent(0.88), .paragraphStyle: para])
            _ = append(partial, [.font: bodyFont, .foregroundColor: ink.withAlphaComponent(0.35), .paragraphStyle: para])
        }
        return TranscriptDoc(text: out, turns: turns, words: words, unheard: unheard, speakers: speakers, indent: indent,
                             looks: looks + ["\(scale)"])
    }

    /// The turn holding a character.
    func turn(at i: Int) -> Int? { Self.find(turns.map(\.range), i) }
    func word(at i: Int) -> Int? { Self.find(words.map(\.range), i) }

    /// The span holding `i` among ranges in order.
    static func find(_ ranges: [NSRange], _ i: Int) -> Int? {
        var lo = 0, hi = ranges.count - 1
        while lo <= hi {
            let mid = (lo + hi) / 2
            let r = ranges[mid]
            if i < r.location { hi = mid - 1 } else if i >= NSMaxRange(r) { lo = mid + 1 } else { return mid }
        }
        return nil
    }
}

/// The transcript's text view. Selection, copying, scrolling while dragging are NSTextView's own; it adds the speakers'
/// dots, the light on a turn under the pointer, the names that open the namer, and the words that play.
final class TranscriptTextView: NSTextView {
    private(set) var doc = TranscriptDoc(text: NSAttributedString(), turns: [], words: [], unheard: [], speakers: false,
                                         indent: 0, looks: [])
    var scale: CGFloat = 1
    var canName = false
    var play: ((Int) -> Void)?
    var openNamer: ((Int, NSRect) -> Void)?
    /// The word being heard: its line and shown index.
    var current: CurrentWord? {
        didSet { if current != oldValue { needsDisplay = true } }
    }
    /// The turn whose namer is open: it stays lit.
    var named: Int? {
        didSet { if named != oldValue { needsDisplay = true } }
    }
    /// The lines a note was written from, lit for a moment after its 대화 기록에서 보기.
    var marked: Set<String> = [] {
        didSet { if marked != oldValue { needsDisplay = true } }
    }

    /// The header above the text: the date, the title, the people, the tabs.
    let header = TranscriptHeaderHost(rootView: AnyView(EmptyView()))
    /// Where the text starts, under the header; the room under it.
    var top: CGFloat = 0 { didSet { if top != oldValue { invalidateTextContainerOrigin() } } }
    var bottom: CGFloat = 60

    override var textContainerOrigin: NSPoint { NSPoint(x: textContainerInset.width, y: top) }

    /// The text's height for a width, laid out once until the text changes.
    private var measured: (width: CGFloat, height: CGFloat)?
    /// What the shown text was made from (`TranscriptPage`): made again only when one of these changes.
    var made: [AnyHashable]?

    /// Back from `TranscriptViewCache`: nothing of the page it was on (pointer, namer, word heard, selection).
    func reset() {
        hoverTurn = nil
        hoverName = false
        hoverWord = nil
        named = nil
        current = nil
        marked = []
        setSelectedRange(NSRange(location: 0, length: 0))
    }

    private var hoverTurn: Int? { didSet { if hoverTurn != oldValue { needsDisplay = true } } }
    private var hoverName = false { didSet { if hoverName != oldValue { needsDisplay = true } } }
    private var hoverWord: Int? { didSet { if hoverWord != oldValue { needsDisplay = true } } }

    static func make() -> TranscriptTextView {
        let storage = NSTextStorage()
        let lm = NSLayoutManager()
        storage.addLayoutManager(lm)
        let tc = NSTextContainer(size: NSSize(width: 100, height: CGFloat.greatestFiniteMagnitude))
        // its width is set with the page's layout, once per width: tracking the view laid the text out again on every
        // frame change
        tc.widthTracksTextView = false
        tc.lineFragmentPadding = 0
        lm.addTextContainer(tc)
        let v = TranscriptTextView(frame: .zero, textContainer: tc)
        v.isEditable = false
        v.isSelectable = true
        v.isRichText = true
        v.drawsBackground = false
        v.textContainerInset = .zero
        v.isVerticallyResizable = false
        v.isHorizontallyResizable = false
        v.usesFontPanel = false
        v.selectedTextAttributes = [.backgroundColor: NSColor.selectedTextBackgroundColor]
        v.addSubview(v.header)
        return v
    }

    /// Shows a new transcript. A meeting going on only adds to its end: the text before stays laid out, the selection
    /// and the scroll stay where they are.
    func show(_ next: TranscriptDoc) {
        guard let storage = textStorage else { return }
        let old = doc
        doc = next
        let a = old.text.string as NSString, b = next.text.string as NSString
        if old.looks.count <= next.looks.count, Array(next.looks.prefix(old.looks.count)) == old.looks, a.length > 0 {
            var p = 0
            let n = min(a.length, b.length)
            while p < n, a.character(at: p) == b.character(at: p) { p += 1 }
            if p == a.length && p == b.length { return }
            // from the start of the paragraph where they part: its spacing may change as a turn goes on
            let from = a.lineRange(for: NSRange(location: min(p, a.length - 1), length: 0)).location
            let keep = selectedRange()
            storage.replaceCharacters(in: NSRange(location: from, length: a.length - from),
                                      with: next.text.attributedSubstring(from: NSRange(location: from, length: b.length - from)))
            if NSMaxRange(keep) <= storage.length { setSelectedRange(keep) }
        } else {
            storage.setAttributedString(next.text)
        }
        measured = nil
        hoverTurn = nil
        hoverWord = nil
        needsDisplay = true
    }

    /// Its height for a width.
    func height(forWidth w: CGFloat) -> CGFloat {
        guard let lm = layoutManager, let tc = textContainer else { return 0 }
        if let m = measured, m.width == w, tc.containerSize.width == w { return m.height }
        if tc.containerSize.width != w { tc.containerSize = NSSize(width: w, height: .greatestFiniteMagnitude) }
        lm.ensureLayout(for: tc)
        let h = ceil(lm.usedRect(for: tc).height)
        measured = (w, h)
        return h
    }

    // MARK: places

    /// The word being heard, in the view.
    var heardRect: NSRect? {
        guard let c = current, let w = doc.words.firstIndex(where: { $0.line == c.line && $0.index == c.word }) else { return nil }
        return rect(doc.words[w].range)
    }

    /// A line's words, one box per row they wrap over; empty when the line has no words on the page.
    func lineBoxes(_ key: String) -> [NSRect] {
        let ws = doc.words.filter { $0.line == key }
        guard let lm = layoutManager, let tc = textContainer, let first = ws.first, let last = ws.last else { return [] }
        let chars = NSRange(location: first.range.location, length: NSMaxRange(last.range) - first.range.location)
        let glyphs = lm.glyphRange(forCharacterRange: chars, actualCharacterRange: nil)
        var boxes: [NSRect] = []
        lm.enumerateEnclosingRects(forGlyphRange: glyphs, withinSelectedGlyphRange: NSRange(location: NSNotFound, length: 0),
                                   in: tc) { r, _ in
            boxes.append(r.offsetBy(dx: self.textContainerOrigin.x, dy: self.textContainerOrigin.y))
        }
        return boxes
    }

    private func rect(_ r: NSRange) -> NSRect {
        guard let lm = layoutManager, let tc = textContainer, r.length > 0 else { return .zero }
        let g = lm.glyphRange(forCharacterRange: r, actualCharacterRange: nil)
        return lm.boundingRect(forGlyphRange: g, in: tc).offsetBy(dx: textContainerOrigin.x, dy: textContainerOrigin.y)
    }

    /// The first line of a range: its line fragment, where a turn's dot and a tag sit.
    private func lineRect(at i: Int) -> NSRect {
        guard let lm = layoutManager else { return .zero }
        let g = lm.glyphIndexForCharacter(at: i)
        return lm.lineFragmentRect(forGlyphAt: g, effectiveRange: nil).offsetBy(dx: textContainerOrigin.x, dy: textContainerOrigin.y)
    }

    /// The box a turn lights in: from its header to its last line, a little wider than the text.
    private func box(_ t: TranscriptDoc.Turn) -> NSRect {
        guard let lm = layoutManager, t.range.length > 0, let length = textStorage?.length, length > 0 else { return .zero }
        // the last turn's range counts the line break the text drops at its end
        let last = min(NSMaxRange(t.range), length) - 1
        let top = lineRect(at: t.range.location).minY
        let g = lm.glyphIndexForCharacter(at: last)
        let used = lm.lineFragmentUsedRect(forGlyphAt: g, effectiveRange: nil).offsetBy(dx: 0, dy: textContainerOrigin.y)
        let w = textContainer?.containerSize.width ?? bounds.width
        return NSRect(x: textContainerOrigin.x - 12, y: top - 10 * scale, width: w + 24, height: used.maxY - top + 20 * scale)
    }

    private func charIndex(at p: NSPoint) -> Int? {
        guard let lm = layoutManager, let tc = textContainer else { return nil }
        let q = NSPoint(x: p.x - textContainerOrigin.x, y: p.y - textContainerOrigin.y)
        var fraction: CGFloat = 0
        let g = lm.glyphIndex(for: q, in: tc, fractionOfDistanceThroughGlyph: &fraction)
        guard lm.boundingRect(forGlyphRange: NSRange(location: g, length: 1), in: tc).insetBy(dx: -2, dy: -2).contains(q) else { return nil }
        return lm.characterIndexForGlyph(at: g)
    }

    /// The ⌄ beside a name: its gap from the name and its width. The name is followed by three en spaces (19.5 at 13 pt)
    /// so the time stays clear of it and of the fill around both.
    private var chevronGap: CGFloat { 4 * scale }
    private var chevronWidth: CGFloat { 10 * scale }

    /// The middle of a name's letters, up and down: the line is 22 tall and the letters sit low in it.
    private func nameMid(_ n: NSRange) -> CGFloat {
        guard let lm = layoutManager else { return 0 }
        let g = lm.glyphIndexForCharacter(at: n.location)
        let line = lm.lineFragmentRect(forGlyphAt: g, effectiveRange: nil)
        let baseline = line.minY + lm.location(forGlyphAt: g).y + textContainerOrigin.y
        return baseline - 0.36 * 13 * scale
    }

    /// The name and its ⌄: where a click opens the namer, filled while the pointer is on it.
    private func nameRect(_ t: TranscriptDoc.Turn) -> NSRect? {
        guard let n = t.name else { return nil }
        let r = rect(n)
        let h = 22 * scale
        return NSRect(x: r.minX - 6 * scale, y: nameMid(n) - h / 2, width: r.width + 6 * scale + chevronGap + chevronWidth + 4 * scale, height: h)
    }

    // MARK: drawing

    override func drawBackground(in dirty: NSRect) {
        super.drawBackground(in: dirty)
        guard let lm = layoutManager, let tc = textContainer, !doc.turns.isEmpty else { return }
        let ink = NSColor(Theme.ink)
        let visible = NSRect(x: 0, y: dirty.minY - 40, width: bounds.width, height: dirty.height + 80)
        let glyphs = lm.glyphRange(forBoundingRect: visible.offsetBy(dx: 0, dy: -textContainerOrigin.y), in: tc)
        let chars = lm.characterRange(forGlyphRange: glyphs, actualGlyphRange: nil)
        guard let first = doc.turn(at: chars.location) ?? doc.turns.firstIndex(where: { NSMaxRange($0.range) > chars.location }) else { return }
        var i = first
        while i < doc.turns.count, doc.turns[i].range.location <= NSMaxRange(chars) {
            let t = doc.turns[i]
            let lit = canName && t.name != nil && (hoverTurn == i || named == i)
            let fill = (t.question ? 0.03 : 0) + (lit ? 0.03 : 0)
            if fill > 0 {
                ink.withAlphaComponent(fill).setFill()
                NSBezierPath(roundedRect: box(t), xRadius: 10, yRadius: 10).fill()
            }
            if let name = t.nameText {
                let line = lineRect(at: t.range.location)
                drawDot(name: name, key: t.key, color: t.color, at: NSPoint(x: textContainerOrigin.x, y: line.minY + scale))
            }
            if let n = nameRect(t), canName, t.name != nil {
                if hoverName && hoverTurn == i || named == i {
                    NSColor(Theme.fillHover).setFill()
                    NSBezierPath(roundedRect: n, xRadius: 6, yRadius: 6).fill()
                }
                if lit, let image = NSImage(systemSymbolName: "chevron.down", accessibilityDescription: nil)?
                    .withSymbolConfiguration(.init(pointSize: 9 * scale, weight: .bold)) {
                    let r = rect(t.name!)
                    let size = image.size
                    let at = NSRect(x: r.maxX + chevronGap + (chevronWidth - size.width) / 2, y: nameMid(t.name!) - size.height / 2,
                                    width: size.width, height: size.height)
                    image.isTemplate = true
                    let tinted = NSImage(size: size, flipped: false) { b in
                        image.draw(in: b)
                        ink.withAlphaComponent(0.5).set()
                        b.fill(using: .sourceAtop)
                        return true
                    }
                    tinted.draw(in: at)
                }
            }
            if let tag = t.tag, let lm = layoutManager {
                let r = rect(tag)
                // centred on the tag's letters from its baseline: the line fragment also holds the space under the
                // turn (`boxedGap`), so its middle sits below the word
                let g = lm.glyphIndexForCharacter(at: tag.location)
                let baseline = lineRect(at: tag.location).minY + lm.location(forGlyphAt: g).y
                let mid = baseline - 0.36 * 10.5 * scale
                let h = 18 * scale
                ink.withAlphaComponent(0.06).setFill()
                NSBezierPath(roundedRect: NSRect(x: r.minX - 6, y: mid - h / 2, width: r.width + 12, height: h),
                             xRadius: h / 2, yRadius: h / 2).fill()
            }
            i += 1
        }
        func wordBox(_ w: Int?, _ color: NSColor) {
            guard let w, doc.words.indices.contains(w) else { return }
            color.setFill()
            NSBezierPath(roundedRect: rect(doc.words[w].range).insetBy(dx: -2, dy: -1), xRadius: 4, yRadius: 4).fill()
        }
        if !marked.isEmpty {
            NSColor(Theme.speakers[0]).withAlphaComponent(0.16).setFill()
            for key in marked {
                for r in lineBoxes(key) { NSBezierPath(roundedRect: r.insetBy(dx: -4, dy: -2), xRadius: 5, yRadius: 5).fill() }
            }
        }
        let heard = current.flatMap { c in doc.words.firstIndex { $0.line == c.line && $0.index == c.word } }
        wordBox(heard, NSColor(Theme.speakers[0]).withAlphaComponent(0.16))
        if play != nil, hoverWord != heard, let w = hoverWord, doc.words[w].ms != nil { wordBox(w, ink.withAlphaComponent(0.07)) }
    }

    private func drawDot(name: String, key: String?, color: Int, at p: NSPoint) {
        let size = 26 * scale
        let hue = NSColor(Theme.speaker(color))
        let r = NSRect(x: p.x, y: p.y, width: size, height: size)
        hue.withAlphaComponent(0.14).setFill()
        NSBezierPath(ovalIn: r).fill()
        hue.withAlphaComponent(0.22).setStroke()
        let ring = NSBezierPath(ovalIn: r.insetBy(dx: 0.25, dy: 0.25))
        ring.lineWidth = 0.5
        ring.stroke()
        let mark = SavedMeeting.speakerMark(name: name, key: key) as NSString
        let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: size * 0.46, weight: .bold), .foregroundColor: hue]
        let s = mark.size(withAttributes: attrs)
        mark.draw(at: NSPoint(x: r.midX - s.width / 2, y: r.midY - s.height / 2), withAttributes: attrs)
    }

    override func draw(_ dirty: NSRect) {
        super.draw(dirty)
        // "▶ 6:12" over the word under the pointer
        guard play != nil, let w = hoverWord, let ms = doc.words[w].ms else { return }
        let label = "▶ \(RecordingPlayer.timeLabel(ms))" as NSString
        let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.monospacedDigitSystemFont(ofSize: 10 * scale, weight: .medium),
                                                    .foregroundColor: NSColor(Theme.onStrong)]
        let s = label.size(withAttributes: attrs)
        let word = rect(doc.words[w].range)
        let h = 16 * scale
        let chip = NSRect(x: word.minX - 2, y: word.minY - h - 3, width: s.width + 10, height: h)
        NSColor(Theme.ink).setFill()
        NSBezierPath(roundedRect: chip, xRadius: 4, yRadius: 4).fill()
        label.draw(at: NSPoint(x: chip.minX + 5, y: chip.midY - s.height / 2), withAttributes: attrs)
    }

    // MARK: the pointer

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for a in trackingAreas where a.owner === self && a.userInfo?["turns"] != nil { removeTrackingArea(a) }
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeInActiveApp, .inVisibleRect],
                                       owner: self, userInfo: ["turns": true]))
    }

    /// Whether the pointer is on the text itself, not on something floating over it (the jump buttons, the player):
    /// the text view's tracking still sees the pointer there, and its I-beam and hover would show through them.
    private func onText(_ e: NSEvent) -> Bool {
        guard let root = window?.contentView?.superview ?? window?.contentView, let hit = root.hitTest(e.locationInWindow) else { return false }
        return hit === self
    }

    override func cursorUpdate(with event: NSEvent) {
        if onText(event) { super.cursorUpdate(with: event) } else { NSCursor.arrow.set() }
    }

    override func mouseMoved(with event: NSEvent) {
        guard onText(event) else {
            NSCursor.arrow.set()
            mouseExited(with: event)
            return
        }
        super.mouseMoved(with: event)
        let p = convert(event.locationInWindow, from: nil)
        // the turn under the pointer: the one holding the nearest letter, if its box holds the pointer (not a search
        // through every turn on each move)
        let near = layoutManager.flatMap { lm in textContainer.map { tc in
            lm.characterIndex(for: NSPoint(x: p.x - textContainerOrigin.x, y: p.y - textContainerOrigin.y), in: tc,
                              fractionOfDistanceBetweenInsertionPoints: nil)
        } }
        let turn = near.flatMap { doc.turn(at: min($0, max(doc.text.length - 1, 0))) }.flatMap { box(doc.turns[$0]).contains(p) ? $0 : nil }
        hoverTurn = turn
        hoverName = turn.flatMap { nameRect(doc.turns[$0]) }?.contains(p) ?? false
        hoverWord = play == nil ? nil : charIndex(at: p).flatMap { doc.word(at: $0) }
        if hoverName && canName { NSCursor.arrow.set() }
    }

    override func mouseExited(with event: NSEvent) {
        hoverTurn = nil
        hoverName = false
        hoverWord = nil
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if canName, event.clickCount == 1, !event.modifierFlags.contains(.shift),
           let i = doc.turns.firstIndex(where: { nameRect($0)?.contains(p) ?? false }), let n = nameRect(doc.turns[i]) {
            openNamer?(i, n)
            return
        }
        let before = charIndex(at: p)
        super.mouseDown(with: event)
        // a click (no drag, no double click) on a word or an unheard line plays from there
        guard event.clickCount == 1, selectedRange().length == 0, let play, let at = before else { return }
        if let w = doc.word(at: at), let ms = doc.words[w].ms {
            play(ms)
        } else if let u = TranscriptDoc.find(doc.unheard.map(\.range), at) {
            play(doc.unheard[u].ms)
        }
    }

    override func resetCursorRects() {
        super.resetCursorRects()
        guard canName else { return }
        for t in doc.turns { if let n = nameRect(t) { addCursorRect(n, cursor: .arrow) } }
    }
}

/// The 대화 기록 tab's page is the text view itself, the scroll view's document, with the header (SwiftUI) sitting in it
/// above the text: as the document, a text view draws only what is on screen (a plain subview drew all 88,000 points
/// of a long meeting at once, 0.7 s), and its own drag scrolling reaches the scroll view.
extension TranscriptTextView {
    /// Lays the page out for the scroll view's width: the column as on the 정리 tab (792 at most, 56 in from each side),
    /// the text view across the whole width so a drag starts in the margins too.
    func layoutPage(width: CGFloat) {
        let column = min(792, width)
        let x = (width - column) / 2 + 56
        let inner = max(column - 112, 100)
        // at the column's width: `fittingSize` laid the header out unwrapped, so a title or people that wrap in a
        // narrower column (the main window's panel open) made it taller than measured and its last line (녹음하지 않은
        // 회의예요, the tabs) drew over the first turns
        let h = ceil(header.height(forWidth: inner))
        header.frame = NSRect(x: x, y: 14, width: inner, height: h)
        top = 14 + h
        if textContainerInset.width != x { textContainerInset = NSSize(width: x, height: 0) }
        let th = height(forWidth: inner)
        let size = NSSize(width: width, height: top + th + bottom)
        if frame.size != size { setFrameSize(size) }
    }
}

private struct LayoutProbeKey: EnvironmentKey {
    static let defaultValue = false
}

extension EnvironmentValues {
    /// True in a copy of a view made only to measure it (`TranscriptHeaderHost`): views with side effects (a field
    /// taking focus, an event monitor) draw a still stand-in of the same size there instead.
    var layoutProbe: Bool {
        get { self[LayoutProbeKey.self] }
        set { self[LayoutProbeKey.self] = newValue }
    }
}

/// The header in the transcript page: says when its content changed size on its own (a note appearing, a banner
/// going), so the text under it moves too. Once per turn of the run loop, however many changes came.
final class TranscriptHeaderHost: NSHostingView<AnyView> {
    var resized: (() -> Void)?
    private var pending = false
    /// Lays the same content out off screen: a hosting view's own `fittingSize` ignores any width and gave the
    /// unwrapped height. The copy is marked `layoutProbe` (no title field, no click monitor in it) and dropped right
    /// after measuring, so no second live header stays around to react to clicks or model changes.
    private lazy var measurer = NSHostingController(rootView: AnyView(EmptyView()))

    /// The header's height laid out at `width`, wrapping as it will on screen.
    func height(forWidth width: CGFloat) -> CGFloat {
        measurer.rootView = AnyView(rootView.environment(\.layoutProbe, true))
        defer { measurer.rootView = AnyView(EmptyView()) }
        return measurer.sizeThatFits(in: NSSize(width: width, height: .greatestFiniteMagnitude)).height
    }

    override func invalidateIntrinsicContentSize() {
        super.invalidateIntrinsicContentSize()
        guard resized != nil, !pending else { return }
        pending = true
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated {
                self?.pending = false
                self?.resized?()
            }
        }
    }
}

/// Where the transcript tab is scrolled, for its 맨 위로 · 맨 아래로, and the way to get there.
@Observable
@MainActor
final class TranscriptScroll {
    var shown = PageSpot.Shown()
    /// Where the word being heard is against the screen: -1 above it, 1 below, 0 on it (or nothing heard).
    var heard = 0
    @ObservationIgnored var jump: ((String) -> Void)?
    /// Scrolls to the word being heard.
    @ObservationIgnored var toHeard: (() -> Void)?
    /// Lines to show once the page is laid out: a note's 대화 기록에서 보기 switches to this tab and leaves its lines here.
    @ObservationIgnored var pending: [String]?
}

struct TranscriptPage<Header: View>: NSViewRepresentable {
    let meeting: SavedMeeting
    let scroll: TranscriptScroll
    let people: [Person]
    let calendar: Calendar
    let editing: SpeakerEditing?
    let playback: TranscriptPlayback?
    let bottom: CGFloat
    /// The sentence being said, in a meeting going on (Soniox main path).
    var live: LiveLine? = nil
    /// Translations under the words (docs/30): 번역 함께 / 원문만 on the tab row.
    var translations: Bool = true
    @ViewBuilder let header: () -> Header

    @Environment(\.textScale) private var scale

    final class Coordinator: NSObject, NSPopoverDelegate {
        var parent: TranscriptPage
        weak var scroll: NSScrollView?
        weak var page: TranscriptTextView?
        var popover: NSPopover?
        var focus: PlaybackFocus?
        /// Counts the lightings of a note's lines (`showPending`).
        var markTurn = 0

        init(_ parent: TranscriptPage) { self.parent = parent }

        @objc func resized() { relayout(); moved(); MainActor.assumeIsolated { showPending() } }

        /// The page against the visible part, for the jump buttons, whenever it scrolls or grows.
        @objc func moved() {
            guard let scroll, let page else { return }
            let clip = scroll.contentView
            let next = PageSpot.Shown(content: CGRect(x: 0, y: -clip.bounds.minY, width: page.frame.width, height: page.frame.height),
                                      viewport: clip.bounds.height)
            let state = parent.scroll
            DispatchQueue.main.async { if state.shown != next { state.shown = next } }
            checkHeard()
        }

        /// Whether the word being heard is off the screen, and which way: the 지금 듣는 곳으로 button shows then.
        func checkHeard() {
            guard let scroll, let page else { return }
            let seen = scroll.contentView.bounds
            var next = 0
            if let r = page.heardRect {
                // the player covers the bottom of the page
                if r.maxY < seen.minY { next = -1 } else if r.minY > seen.maxY - 70 { next = 1 }
            }
            let state = parent.scroll
            DispatchQueue.main.async { if state.heard != next { state.heard = next } }
        }

        /// The word being heard a third of the way down the screen.
        func toHeard() {
            guard let scroll, let page, let r = page.heardRect else { return }
            let clip = scroll.contentView
            let y = min(max(r.midY - clip.bounds.height / 3, 0), max(page.frame.height - clip.bounds.height, 0))
            clip.scroll(to: NSPoint(x: 0, y: y))
            scroll.reflectScrolledClipView(clip)
        }

        /// The lines left in `pending`, once the page has its width: the first a third of the way down, all of them lit for
        /// a moment. Lines not on the page leave the page where it is.
        @MainActor func showPending() {
            guard let keys = parent.scroll.pending, let scroll, let page, scroll.contentView.bounds.width > 0,
                  page.frame.height > 0 else { return }
            parent.scroll.pending = nil
            let boxes = keys.flatMap(page.lineBoxes)
            guard let top = boxes.map(\.minY).min() else { return }
            let clip = scroll.contentView
            let y = min(max(top - clip.bounds.height / 3, 0), max(page.frame.height - clip.bounds.height, 0))
            clip.scroll(to: NSPoint(x: 0, y: y))
            scroll.reflectScrolledClipView(clip)
            page.marked = Set(keys)
            // a later click lights again: only the last one's timer puts it out
            markTurn += 1
            let turn = markTurn
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.4) { [weak self, weak page] in
                if self?.markTurn == turn { page?.marked = [] }
            }
        }

        /// Straight to the top or the bottom, without sliding.
        func jump(_ to: String) {
            guard let scroll, let page else { return }
            let clip = scroll.contentView
            let y = to == PageSpot.top ? 0 : max(page.frame.height - clip.bounds.height, 0)
            clip.scroll(to: NSPoint(x: 0, y: y))
            scroll.reflectScrolledClipView(clip)
        }

        func relayout() {
            // not laid out yet: a page laid out at no width was the whole text laid out again at once, then once more
            // for the real width (twice the time of a switch); the scroll view's first size comes through resized()
            guard let scroll, let page, scroll.contentView.bounds.width > 0 else { return }
            page.layoutPage(width: scroll.contentView.bounds.width)
        }

        @MainActor func openNamer(turn i: Int, at r: NSRect) {
            guard let page, let editing = parent.editing, let target = page.doc.turns[i].target else { return }
            popover?.close()
            let p = NSPopover()
            p.behavior = .transient
            p.delegate = self
            let done = { [weak p] in p?.close() }
            let namer = Namer(meeting: parent.meeting, target: target, people: parent.people, editing: editing) { done() }
                .environment(\.textScale, parent.scale)
            p.contentViewController = NSHostingController(rootView: namer)
            page.named = i
            p.show(relativeTo: r, of: page, preferredEdge: .maxY)
            popover = p
        }

        func popoverDidClose(_ notification: Notification) {
            MainActor.assumeIsolated { page?.named = nil }
            popover = nil
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSScrollView()
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.scrollerStyle = .overlay
        let page = TranscriptViewCache.take(meeting.id) ?? TranscriptTextView.make()
        scroll.documentView = page
        scroll.contentView.postsFrameChangedNotifications = true
        scroll.contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(context.coordinator, selector: #selector(Coordinator.resized),
                                               name: NSView.frameDidChangeNotification, object: scroll.contentView)
        NotificationCenter.default.addObserver(context.coordinator, selector: #selector(Coordinator.moved),
                                               name: NSView.boundsDidChangeNotification, object: scroll.contentView)
        context.coordinator.scroll = scroll
        context.coordinator.page = page
        let c = context.coordinator
        page.openNamer = { [weak c] i, r in c?.openNamer(turn: i, at: r) }
        page.header.resized = { [weak c] in c?.relayout() }
        self.scroll.jump = { [weak c] to in c?.jump(to) }
        self.scroll.toHeard = { [weak c] in c?.toHeard() }
        update(scroll, context: context)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) { update(scroll, context: context) }

    /// The text view outlives the page: back on this meeting, its text and layout are already made.
    static func dismantleNSView(_ scroll: NSScrollView, coordinator: Coordinator) {
        coordinator.popover?.close()
        guard let page = coordinator.page else { return }
        scroll.documentView = nil
        TranscriptViewCache.keep(page, for: coordinator.parent.meeting.id)
    }

    private func update(_ scroll: NSScrollView, context: Context) {
        let c = context.coordinator
        c.parent = self
        guard let page = c.page else { return }
        page.header.rootView = AnyView(header().environment(\.textScale, scale))
        page.bottom = bottom
        let unheard = playback?.unheard ?? []
        let key: [AnyHashable] = [meeting.transcript.map(\.key), meeting.transcript.map(\.text), meeting.transcript.map(\.question),
                                  meeting.transcript.map { $0.voice ?? "" }, meeting.transcript.map { $0.name ?? "" },
                                  meeting.transcript.map { "\($0.audioAt ?? -1)|\($0.words?.count ?? -1)" },
                                  meeting.voices.map { $0.map { "\($0.key)|\($0.name ?? "")|\($0.person ?? "")|\($0.same ?? "")" } } ?? [],
                                  people.map { "\($0.id)|\($0.name)|\($0.color)" },
                                  meeting.questions.map { "\($0.question)|\($0.answer ?? "")|\($0.unanswered)" },
                                  unheard.map { "\($0.start)-\($0.end)" }, scale, live?.held ?? [], live?.said ?? "", live?.partial ?? "",
                                  translations, meeting.transcript.map { "\($0.translation ?? "")|\($0.translatedFrom ?? "")" }]
        if page.made != key {
            page.made = key
            page.scale = CGFloat(scale)
            let doc = TranscriptDoc.make(meeting: meeting, people: people, calendar: calendar, unheard: unheard, scale: CGFloat(scale),
                                         live: live, translations: translations)
            page.show(doc)
            page.window?.invalidateCursorRects(for: page)
        }
        page.canName = editing != nil
        let live = playback.map { !$0.dimmed } ?? false
        page.play = live ? playback?.play : nil
        page.alphaValue = playback?.dimmed == true ? 0.4 : 1
        if c.focus !== playback?.focus {
            c.focus = playback?.focus
            playback?.focus?.onChange = { [weak page, weak c] w in
                page?.current = w
                c?.checkHeard()
            }
            page.current = playback?.focus?.current
        }
        c.relayout()
        c.moved()
        // the first update of a page just switched to may come before its layout: then resized() shows them
        DispatchQueue.main.async { MainActor.assumeIsolated { c.showPending() } }
    }
}

/// The transcript views of the last few meetings shown, text made and laid out, so going back to one does not make
/// and lay out all its text again (a long meeting took 0.1 s and more). Only in memory; gone when the app quits.
@MainActor
enum TranscriptViewCache {
    static let limit = 4
    private static var views: [(id: String, view: TranscriptTextView)] = []

    static func take(_ id: String) -> TranscriptTextView? {
        guard let i = views.firstIndex(where: { $0.id == id }) else { return nil }
        let v = views.remove(at: i).view
        v.reset()
        return v
    }

    static func keep(_ view: TranscriptTextView, for id: String) {
        // the header held the old page's views; the page that takes it sets its own
        view.header.rootView = AnyView(EmptyView())
        view.header.resized = nil
        view.play = nil
        view.openNamer = nil
        views.removeAll { $0.id == id }
        views.append((id, view))
        if views.count > limit { views.removeFirst(views.count - limit) }
    }

    /// A deleted meeting's words do not stay in memory.
    static func keepOnly(_ ids: Set<String>) { views.removeAll { !ids.contains($0.id) } }
}
