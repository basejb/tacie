import AVFoundation
import Foundation
import Observation

/// Whether a meeting's recording can be played.
enum RecordingState: Equatable {
    case none                 // the meeting was never recorded
    case deleted              // recorded once; the sound is gone (or does not open)
    case available([URL])     // the files to play back to back (one final file, or the parts a crash left)

    /// The files of `recording` that open, in order; empty when nothing plays.
    static func files(for recording: Recording?, audio: URL) -> [URL] {
        recording == .kept ? MeetingRecorder.playableFiles(for: audio) : []
    }

    /// When a meeting page checks the sound again: live or not, the record's recording, and the files' revision (the
    /// join after a meeting ends moves its files but often leaves the record as it was).
    static func checkKey(live: Bool, meeting: SavedMeeting, audioRevision: Int) -> String {
        "\(live) \(meeting.recording?.rawValue ?? "none") \(meeting.recordingSeconds ?? -1) \(audioRevision)"
    }

    /// Total length of the files in seconds.
    static func seconds(of files: [URL]) -> Double {
        files.reduce(0) { sum, u in
            guard let f = try? AVAudioFile(forReading: u), f.processingFormat.sampleRate > 0 else { return sum }
            return sum + Double(f.length) / f.processingFormat.sampleRate
        }
    }
}

/// Plays one meeting's recording from any millisecond, so a tapped word starts the sound there.
/// Several files (parts after a crash) play as one timeline.
@Observable
@MainActor
final class RecordingPlayer {
    private(set) var meetingID: String?
    private(set) var playing = false
    private(set) var positionMs = 0
    private(set) var durationMs = 0
    /// 1, 1.5 or 2 (`nextRate()`).
    var rate: Float = 1 {
        didSet { if playing { player?.rate = rate } }
    }
    /// 0...1; tests set 0 so nothing is heard.
    var volume: Float = 1 {
        didSet { player?.volume = volume }
    }

    @ObservationIgnored private var player: AVPlayer?
    @ObservationIgnored nonisolated(unsafe) private var ticker: Timer?
    @ObservationIgnored nonisolated(unsafe) private var endObserver: NSObjectProtocol?
    @ObservationIgnored nonisolated(unsafe) private var startObserver: NSObjectProtocol?
    @ObservationIgnored private var generation = 0

    init() {
        startObserver = NotificationCenter.default.addObserver(forName: .meetingStarted, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.stop() }
        }
    }

    deinit {
        if let startObserver { NotificationCenter.default.removeObserver(startObserver) }
        let (t, e) = (ticker, endObserver)
        t?.invalidate()
        if let e { NotificationCenter.default.removeObserver(e) }
    }

    func nextRate() {
        rate = rate < 1.25 ? 1.5 : rate < 1.75 ? 2 : 1
    }

    /// Starts `meeting`'s recording at `fromMs`. Another meeting's sound stops first.
    func play(meeting: String, urls: [URL], fromMs: Int) {
        stop()
        guard !urls.isEmpty else { return }
        generation += 1
        let mine = generation
        meetingID = meeting
        playing = true
        positionMs = max(0, fromMs)
        durationMs = Int((RecordingState.seconds(of: urls) * 1000).rounded())
        Task { [weak self] in
            guard let item = await Self.item(for: urls) else { await MainActor.run { if self?.generation == mine { self?.stop() } }; return }
            guard let self, self.generation == mine else { return }
            let p = AVPlayer(playerItem: item)
            p.volume = self.volume
            p.automaticallyWaitsToMinimizeStalling = false
            self.player = p
            self.endObserver = NotificationCenter.default.addObserver(forName: .AVPlayerItemDidPlayToEndTime, object: item, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.reachedEnd() }
            }
            let start = CMTime(value: CMTimeValue(self.positionMs), timescale: 1000)
            await p.seek(to: start, toleranceBefore: .zero, toleranceAfter: .zero)
            guard self.generation == mine else { return }
            if self.playing { p.rate = self.rate; self.startTicker() }
        }
    }

    func play(meeting: String, url: URL, fromMs: Int) { play(meeting: meeting, urls: [url], fromMs: fromMs) }

    func toggle() {
        guard meetingID != nil else { return }
        guard let player else { playing.toggle(); return }   // still loading: the load path honours `playing`
        if playing {
            refresh()
            playing = false
            player.pause()
            stopTicker()
        } else {
            if positionMs >= durationMs { seek(toMs: 0) }
            playing = true
            player.rate = rate
            startTicker()
        }
    }

    func seek(toMs ms: Int) {
        let clamped = min(max(0, ms), durationMs > 0 ? durationMs : max(0, ms))
        positionMs = clamped
        player?.seek(to: CMTime(value: CMTimeValue(clamped), timescale: 1000), toleranceBefore: .zero, toleranceAfter: .zero)
    }

    func stop() {
        generation += 1
        stopTicker()
        if let endObserver { NotificationCenter.default.removeObserver(endObserver) }
        endObserver = nil
        player?.pause()
        player = nil
        playing = false
        meetingID = nil
        positionMs = 0
        durationMs = 0
    }

    // MARK: inside

    private func stopTicker() {
        ticker?.invalidate()
        ticker = nil
    }

    private func reachedEnd() {
        stopTicker()
        playing = false
        positionMs = durationMs
    }

    private func startTicker() {
        ticker?.invalidate()
        ticker = Timer.scheduledTimer(withTimeInterval: 0.03, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.refresh() }
        }
    }

    private func refresh() {
        guard let player, playing else { return }
        let s = player.currentTime().seconds
        if s.isFinite { positionMs = min(max(0, Int((s * 1000).rounded())), durationMs) }
    }

    /// The files laid end to end as one item.
    nonisolated private static func item(for urls: [URL]) async -> AVPlayerItem? {
        let comp = AVMutableComposition()
        guard let track = comp.addMutableTrack(withMediaType: .audio, preferredTrackID: kCMPersistentTrackID_Invalid) else { return nil }
        var at = CMTime.zero
        for u in urls {
            let asset = AVURLAsset(url: u)
            guard let src = try? await asset.loadTracks(withMediaType: .audio).first,
                  let range = try? await src.load(.timeRange),
                  (try? track.insertTimeRange(range, of: src, at: at)) != nil else { continue }
            at = CMTimeAdd(at, range.duration)
        }
        return at > .zero ? AVPlayerItem(asset: comp) : nil
    }

    // MARK: what is being said

    /// The word spoken at `atMs`: the last one that started by then (a line without words: the line, word -1).
    static func currentWord(in lines: [TranscriptLine], atMs: Int) -> (lineKey: String, wordIndex: Int)? {
        var best: (key: String, index: Int, start: Int)?
        for l in lines {
            if let words = l.words, !words.isEmpty {
                for (i, w) in words.enumerated() where w.start <= atMs && w.start >= (best?.start ?? Int.min) {
                    best = (l.key, i, w.start)
                }
            } else if let a = l.audioAt, a <= atMs, a >= (best?.start ?? Int.min) {
                best = (l.key, -1, a)
            }
        }
        guard let best else { return nil }
        return (best.key, best.index)
    }

    /// Who is speaking at `atMs` (the voice of the line it falls in).
    static func speaker(in lines: [TranscriptLine], atMs: Int) -> String? {
        lines.filter { ($0.audioAt ?? Int.max) <= atMs }.max { ($0.audioAt ?? 0) < ($1.audioAt ?? 0) }?.voice
    }
}
