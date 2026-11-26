import Foundation

/// What `read_transcript` returns: lines of one saved meeting, around a decision or a line, or page by page (docs/38 4절).
enum MCPTranscript {
    static let maxLines = 60
    static let defaultRadius = 6
    static let maxRadius = 20

    static func read(_ m: SavedMeeting, around: String?, before: Int?, after: Int?, offset: Int?, limit: Int?,
                     name: (TranscriptLine) -> String, zone: TimeZone = .current) -> [String: Any] {
        let lines = m.transcript.filter { !$0.isSoundOnly }
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = zone
        f.dateFormat = "HH:mm"
        func shape(_ l: TranscriptLine) -> [String: Any] {
            ["key": l.key, "at": f.string(from: l.at), "speaker": name(l), "text": l.text]
        }

        var missed = false
        if let around = around?.trimmingCharacters(in: .whitespacesAndNewlines), !around.isEmpty {
            let centers = Self.centers(around, ledger: m.ledger, lines: lines)
            if !centers.isEmpty {
                let b = min(max(before ?? defaultRadius, 0), maxRadius)
                let a = min(max(after ?? defaultRadius, 0), maxRadius)
                var keep = Set<Int>()
                for c in centers { for i in max(0, c - b)...min(lines.count - 1, c + a) { keep.insert(i) } }
                let all = keep.sorted()
                var out: [String: Any] = ["lines": all.prefix(maxLines).map { shape(lines[$0]) }]
                if all.count > maxLines { out["truncated"] = true }
                return out
            }
            missed = true
        }

        let start = min(max(offset ?? 0, 0), lines.count)
        let n = min(max(limit ?? maxLines, 1), maxLines)
        let end = min(start + n, lines.count)
        var out: [String: Any] = ["lines": lines[start..<end].map(shape)]
        if end < lines.count { out["next_offset"] = end }
        // `around` named nothing here: say so, so the AI app does not read the opening as the context it asked for
        if missed { out["found"] = false }
        return out
    }

    /// Where a window is centered for what the AI app named: a line key gives that line; a ledger entry's text (exact,
    /// then contained) gives the lines it was written from; otherwise the line sharing the most words with it, when it
    /// shares at least `minSharedWords` (a paraphrase or a phrase from the talk). Nothing found: no centers.
    private static func centers(_ around: String, ledger: [LedgerEntry], lines: [TranscriptLine]) -> [Int] {
        if let i = lines.firstIndex(where: { $0.key == around }) { return [i] }
        let entry = ledger.first { $0.text == around }
            ?? ledger.first { $0.text.range(of: around, options: [.caseInsensitive, .diacriticInsensitive]) != nil }
        if let entry {
            let c = centers(for: entry, in: lines)
            if !c.isEmpty { return c }
        }
        if let at = bestOverlap(words(around), in: lines, atLeast: minSharedWords) { return [at] }
        return []
    }

    static let minSharedWords = 2

    /// The line sharing the most words with `words` (the first on a tie), if it shares at least `atLeast`.
    private static func bestOverlap(_ words: Set<String>, in transcript: [TranscriptLine], atLeast: Int) -> Int? {
        var best = 0, at: Int?
        for (i, l) in transcript.enumerated() {
            let n = Self.words(l.text).intersection(words).count
            if n > best { best = n; at = i }
        }
        return best >= atLeast ? at : nil
    }

    /// The lines a decision was written from (`refs`); without refs the line sharing the most words with it.
    static func centers(for entry: LedgerEntry, in transcript: [TranscriptLine]) -> [Int] {
        var centers = (entry.refs ?? []).compactMap { key in transcript.firstIndex { $0.key == key } }
        if centers.isEmpty, let at = bestOverlap(words(entry.text), in: transcript, atLeast: 1) { centers = [at] }
        return centers
    }

    private static func words(_ s: String) -> Set<String> {
        Set(s.lowercased().split { !($0.isLetter || $0.isNumber) }.map(String.init).filter { $0.count >= 2 })
    }
}
