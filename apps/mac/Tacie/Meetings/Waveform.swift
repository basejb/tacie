import AVFoundation
import Accelerate
import Foundation

/// The player's waveform: how loud the recording is along its length, and who speaks where (the bars take the
/// speaker's colour).
enum Waveform {
    /// Steps the recording is cut into; the bar draws fewer bars and takes the loudest step of each.
    static let bins = 240

    /// What colours a step.
    enum Tint: Equatable {
        case quiet            // nobody's line covers it
        case voiceless        // a line covers it, but the meeting has no speakers
        case speaker(Int)     // a speaker's palette index
        case unheard          // kept, but its words did not come back
    }

    /// Loudness per step, 0...1, the files end to end. Scaled to the loud end of the meeting (95th percentile) so one
    /// cough does not flatten everything else.
    static func peaks(of files: [URL], bins: Int = bins) -> [Float] {
        let opened = files.compactMap { try? AVAudioFile(forReading: $0) }
        let total = opened.reduce(Int64(0)) { $0 + $1.length }
        guard total > 0, bins > 0 else { return [] }
        var sums = [Float](repeating: 0, count: bins)
        var counts = [Int64](repeating: 0, count: bins)
        var at: Int64 = 0
        let chunk: AVAudioFrameCount = 32_768
        for f in opened {
            guard let buf = AVAudioPCMBuffer(pcmFormat: f.processingFormat, frameCapacity: chunk) else { continue }
            while f.framePosition < f.length {
                guard (try? f.read(into: buf, frameCount: chunk)) != nil, buf.frameLength > 0,
                      let ch = buf.floatChannelData?[0] else { break }
                let n = Int64(buf.frameLength)
                var i: Int64 = 0
                while i < n {
                    let b = Int((at + i) * Int64(bins) / total)
                    let binEnd = (Int64(b + 1) * total + Int64(bins) - 1) / Int64(bins)   // first sample of the next step
                    let run = min(n - i, max(1, binEnd - (at + i)))
                    var sq: Float = 0
                    vDSP_svesq(ch + Int(i), 1, &sq, vDSP_Length(run))
                    sums[min(b, bins - 1)] += sq
                    counts[min(b, bins - 1)] += run
                    i += run
                }
                at += n
            }
        }
        let rms = zip(sums, counts).map { s, c in c > 0 ? (s / Float(c)).squareRoot() : 0 }
        let top = rms.sorted()[Int(Double(rms.count - 1) * 0.95)]
        guard top > 0 else { return rms.map { _ in 0 } }
        return rms.map { min(1, $0 / top) }
    }

    /// Who speaks in each step. A line runs from its start to its last word's end (without words: to the next line's
    /// start). An unheard stretch wins over lines. `color` gives a line's palette index, nil without speakers.
    static func tints(lines: [TranscriptLine], unheard: [AudioSpan], durationMs: Int, bins: Int = bins,
                      color: (TranscriptLine) -> Int?) -> [Tint] {
        guard durationMs > 0, bins > 0 else { return [] }
        let placed = lines.filter { $0.audioAt != nil }.sorted { $0.audioAt! < $1.audioAt! }
        var spans: [(start: Int, end: Int, tint: Tint)] = []
        for (i, l) in placed.enumerated() {
            let start = l.audioAt!
            let next = i + 1 < placed.count ? placed[i + 1].audioAt! : durationMs
            let end = l.words?.last.map { $0.end } ?? next
            spans.append((start, max(start, end), color(l).map(Tint.speaker) ?? .voiceless))
        }
        return (0..<bins).map { b in
            let mid = Int((Double(b) + 0.5) * Double(durationMs) / Double(bins))
            if unheard.contains(where: { $0.start <= mid && mid < $0.end }) { return .unheard }
            return spans.last { $0.start <= mid && mid < $0.end }?.tint ?? .quiet
        }
    }

    /// `count` bars from the steps: each the loudest step it covers, in that step's tint.
    static func bars(peaks: [Float], tints: [Tint], count: Int) -> [(height: Float, tint: Tint)] {
        guard count > 0, !peaks.isEmpty else { return [] }
        return (0..<count).map { j in
            let lo = j * peaks.count / count, hi = max(lo + 1, (j + 1) * peaks.count / count)
            let k = (lo..<min(hi, peaks.count)).max { peaks[$0] < peaks[$1] } ?? lo
            return (peaks[k], k < tints.count ? tints[k] : .quiet)
        }
    }

    /// Peaks already read, by the files (path, size) they came from: reading an hour takes about a second.
    @MainActor private static var cache: [String: [Float]] = [:]

    @MainActor static func cachedPeaks(of files: [URL]) async -> [Float] {
        let key = files.map { u in
            "\(u.path):\(((try? FileManager.default.attributesOfItem(atPath: u.path)[.size]) as? Int) ?? 0)"
        }.joined(separator: "|")
        if let hit = cache[key] { return hit }
        let made = await Task.detached(priority: .utility) { peaks(of: files) }.value
        cache[key] = made
        return made
    }
}
