import Foundation

/// A speaker heard only briefly is usually one of the meeting's people whose voice was not settled yet when that line
/// came (early in the meeting, a laugh, two people at once). Such a speaker joins the nearest speaker who talked enough,
/// when the two voices are close (docs/01 "작은 화자 합치기"): on 110 public recordings it took the one-line speakers
/// down by half or more and made no recording worse. A speaker who said little and sounds like nobody else is someone
/// who spoke briefly, and stays.
///
/// Only with the speaker-grouping model's voices (`diarVector`): the values were measured with it.
enum SmallSpeakers {
    /// Small: under this many seconds of speech, in at most `maxLines` lines.
    static let maxSeconds: Float = 12
    static let maxLines = 3
    /// The cosine (speaker-grouping voices) the nearest speaker must reach.
    static let minCos: Float = 0.5

    /// Small speaker key → the speaker it joins. `lines` counts each speaker's transcript lines.
    static func folds(_ prints: [MeetingPrint], lines: [String: Int]) -> [String: String] {
        let small = { (p: MeetingPrint) in p.seconds < maxSeconds && (lines[p.key] ?? 0) <= maxLines }
        let big = prints.filter { !small($0) && $0.diarVector != nil }
        var out: [String: String] = [:]
        for p in prints where small(p) && (lines[p.key] ?? 0) > 0 && p.person == nil {
            guard let v = p.diarVector else { continue }
            var best: (key: String, cos: Float)?
            for b in big where b.channel == p.channel {
                guard let w = b.diarVector, w.count == v.count else { continue }
                let c = cosine(v, w)
                if c > best?.cos ?? -2 { best = (b.key, c) }
            }
            if let best, best.cos >= minCos { out[p.key] = best.key }
        }
        return out
    }

    private static func cosine(_ a: [Float], _ b: [Float]) -> Float {
        var ab: Float = 0, aa: Float = 0, bb: Float = 0
        for (x, y) in zip(a, b) {
            ab += x * y
            aa += x * x
            bb += y * y
        }
        return aa == 0 || bb == 0 ? 0 : ab / (aa * bb).squareRoot()
    }
}

extension SavedMeeting {
    /// Joins the meeting's small speakers to the speaker each sounds like (`SmallSpeakers`), and lets go of one joined
    /// this way that no longer is small or close. Speakers someone named, linked to a remembered person, or merged by
    /// hand are left as they are, and so is one the engine knew as a remembered person. True when anything changed.
    mutating func foldSmallSpeakers(_ prints: [MeetingPrint]) -> Bool {
        var lines: [String: Int] = [:]
        for l in transcript { if let v = l.voice { lines[v, default: 0] += 1 } }
        let want = SmallSpeakers.folds(prints, lines: lines).mapValues { resolved($0) }
        var changed = false
        var all = voices ?? []
        for i in all.indices where all[i].auto == true && want[all[i].key] != all[i].same {
            all[i].same = nil
            all[i].auto = nil
            changed = true
        }
        voices = all
        for (key, target) in want.sorted(by: { $0.key < $1.key }) {
            let to = target
            guard to != key, resolved(key) == key else { continue }
            let e = voices?.first { $0.key == key }
            guard e?.name == nil, e?.person == nil else { continue }
            var list = voices ?? []
            if let i = list.firstIndex(where: { $0.key == key }) {
                list[i].same = to
                list[i].auto = true
            } else {
                list.append(VoiceEntry(key: key, same: to, auto: true))
            }
            voices = list
            changed = true
        }
        return changed
    }

    /// Lets go of speaker `key` when the app joined it on its own (`foldSmallSpeakers`). True when it did.
    mutating func releaseFolded(_ key: String) -> Bool {
        guard let i = voices?.firstIndex(where: { $0.key == key && $0.auto == true }) else { return false }
        voices?[i].same = nil
        voices?[i].auto = nil
        return true
    }
}
