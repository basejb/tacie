import AVFoundation

/// Writes a meeting's audio (16 kHz mono Float samples) as AAC in CAF files next to the meeting record.
///
/// A CAF+AAC file that is never closed reads back as zero frames (the packet table is written on close; the
/// crash test showed it), so a meeting cut short by a crash would lose everything. The audio is therefore
/// written in parts, `<id>.part-0001.caf`, `<id>.part-0002.caf`, ..., each closed after about a minute.
/// `finish()` joins the parts into `<id>.caf` and removes them. After a crash only the parts are left and
/// at most the last minute (the part that was still open, which reads as empty) is lost.
///
/// `append` is called on the audio thread, so it only hands the samples to a private serial queue.
final class MeetingRecorder {
    static let sampleRate = 16_000.0
    static let partSeconds = 60.0

    private let url: URL
    private let partFrames: Int64
    private let queue = DispatchQueue(label: "ai.tacie.meeting-recorder")
    private let format: AVAudioFormat
    private static let settings: [String: Any] = [   // ~24 kbps: speech stays clear, an hour is about 10 MB
        AVFormatIDKey: kAudioFormatMPEG4AAC,
        AVSampleRateKey: sampleRate,
        AVNumberOfChannelsKey: 1,
        AVEncoderBitRateKey: 24_000,
    ]

    // queue only
    private var file: AVAudioFile?
    private var part = 0
    private var partWritten: Int64 = 0
    private var frames: Int64 = 0
    private var finished = false

    private let lock = NSLock()
    private var written = 0.0   // seconds, readable from any thread

    /// `url` is the final `<id>.caf`. `partSeconds` is only changed by tests. `resuming`: the meeting is taken up again
    /// (이어서 듣기): the sound it has so far becomes the first part (`part-0000`) and the new sound follows it, so
    /// `finish()` joins them into one file again.
    init(url: URL, partSeconds: Double = MeetingRecorder.partSeconds, resuming: Bool = false) throws {
        guard let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: Self.sampleRate, channels: 1, interleaved: false) else {
            throw CocoaError(.fileWriteUnknown)
        }
        self.url = url
        self.format = format
        self.partFrames = Int64(partSeconds * Self.sampleRate)
        if resuming {
            let fm = FileManager.default
            if fm.fileExists(atPath: url.path) {
                try? fm.removeItem(at: Self.partURL(url, 0))
                try fm.moveItem(at: url, to: Self.partURL(url, 0))
            }
            // parts a crash left behind stay before the new ones
            let before = Self.parts(for: url)
            part = before.compactMap { Int($0.deletingPathExtension().lastPathComponent.suffix(4)) }.max() ?? 0
            // `seconds` counts the whole recording, so the meeting's length and new lines' places go on from the old end
            frames = before.reduce(0) { $0 + ((try? AVAudioFile(forReading: $1).length) ?? 0) }
            written = Double(frames) / Self.sampleRate
        }
        try openNextPart()
    }

    func append(_ samples: [Float]) {
        guard !samples.isEmpty else { return }
        queue.async { [self] in
            guard !finished, let file,
                  let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(samples.count)),
                  let channel = buffer.floatChannelData?[0] else { return }
            samples.withUnsafeBufferPointer { channel.update(from: $0.baseAddress!, count: samples.count) }
            buffer.frameLength = AVAudioFrameCount(samples.count)
            do {
                try file.write(from: buffer)
                frames += Int64(samples.count)
                partWritten += Int64(samples.count)
                lock.lock(); written = Double(frames) / Self.sampleRate; lock.unlock()
                if partWritten >= partFrames { try openNextPart() }   // closes the full part
            } catch {
                Log.info("meeting audio write failed: \(error.localizedDescription)")
                self.file = nil   // stop writing; the closed parts stay playable
            }
        }
    }

    /// Seconds written so far.
    var seconds: Double {
        lock.lock(); defer { lock.unlock() }
        return written
    }

    /// Closes the last part (after the queued samples), joins the parts into the final file and returns the seconds written.
    /// Blocks while the whole meeting is decoded and re-encoded: never call it on the main thread, use a background queue.
    @discardableResult
    func finish() -> Double {
        queue.sync {
            guard !finished else { return }
            finished = true
            file = nil   // releasing the AVAudioFile closes it
            join()
        }
        return seconds
    }

    /// Closes the open part (after the queued samples) without joining: only a file close, fast enough for the app's
    /// quit. Every part is then playable (`playableFiles(for:)`); a later `finish()` does nothing.
    @discardableResult
    func close() -> Double {
        queue.sync {
            guard !finished else { return }
            finished = true
            file = nil
        }
        return seconds
    }

    /// Drains the queue so everything appended so far has reached the files, without closing the open part.
    func flushForTesting() {
        queue.sync {}
    }

    // MARK: Files of one recording

    /// The parts of the recording that final file `url` stands for, in order. Empty after `finish()`.
    static func parts(for url: URL) -> [URL] {
        let stem = url.deletingPathExtension().lastPathComponent
        let names = (try? FileManager.default.contentsOfDirectory(atPath: url.deletingLastPathComponent().path)) ?? []
        return names.filter { $0.hasPrefix("\(stem).part-") && $0.hasSuffix(".caf") }.sorted()
            .map { url.deletingLastPathComponent().appendingPathComponent($0) }
    }

    /// What to play: the final file once it exists, otherwise the parts a crash left behind.
    /// A part that was open when the app died reads as empty; skip any file with no frames.
    static func playableFiles(for url: URL) -> [URL] {
        func readable(_ u: URL) -> Bool { ((try? AVAudioFile(forReading: u).length) ?? 0) > 0 }
        if FileManager.default.fileExists(atPath: url.path), readable(url) { return [url] }
        return parts(for: url).filter(readable)   // no final file, or an empty/broken one: use the parts
    }

    /// Every file of this recording: the final one, its parts and a half-made join.
    static func allFiles(for url: URL) -> [URL] {
        var all = parts(for: url)
        all.append(url)
        all.append(joinURL(url))
        return all
    }

    private static func partURL(_ url: URL, _ n: Int) -> URL {
        let stem = url.deletingPathExtension().lastPathComponent
        return url.deletingLastPathComponent().appendingPathComponent("\(stem).part-\(String(format: "%04d", n)).caf")
    }

    private static func joinURL(_ url: URL) -> URL {
        let stem = url.deletingPathExtension().lastPathComponent
        return url.deletingLastPathComponent().appendingPathComponent("\(stem).joining.caf")
    }

    // MARK: queue only

    private func openNextPart() throws {
        file = nil   // close the full one first
        part += 1
        partWritten = 0
        file = try AVAudioFile(forWriting: Self.partURL(url, part), settings: Self.settings,
                               commonFormat: .pcmFormatFloat32, interleaved: false)
    }

    /// Joins the parts into the final file. A single part is just renamed; more are decoded and encoded as one stream
    /// (the parts cannot be glued as packets: each starts with its own encoder lead-in). On any failure the parts stay.
    private func join() {
        let fm = FileManager.default
        let parts = Self.parts(for: url).filter { (try? AVAudioFile(forReading: $0).length) ?? 0 > 0 }
        let empty = Set(Self.parts(for: url)).subtracting(parts)
        defer { for e in empty { try? fm.removeItem(at: e) } }
        guard !parts.isEmpty else { return }
        do {
            try? fm.removeItem(at: url)
            if parts.count == 1 {
                try fm.moveItem(at: parts[0], to: url)
                return
            }
            let temp = Self.joinURL(url)
            try? fm.removeItem(at: temp)
            do {
                let out = try AVAudioFile(forWriting: temp, settings: Self.settings, commonFormat: .pcmFormatFloat32, interleaved: false)
                for p in parts {
                    let input = try AVAudioFile(forReading: p, commonFormat: .pcmFormatFloat32, interleaved: false)
                    while input.framePosition < input.length {
                        guard let buffer = AVAudioPCMBuffer(pcmFormat: input.processingFormat, frameCapacity: 16_000) else { throw CocoaError(.fileReadUnknown) }
                        try input.read(into: buffer)
                        if buffer.frameLength == 0 { break }
                        try out.write(from: buffer)
                    }
                }
            }   // `out` released here: closed
            try fm.moveItem(at: temp, to: url)
            for p in parts { try? fm.removeItem(at: p) }
        } catch {
            Log.info("meeting audio join failed: \(error.localizedDescription)")
            try? fm.removeItem(at: Self.joinURL(url))
        }
    }
}
