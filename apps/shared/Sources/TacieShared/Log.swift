import Foundation

/// Plain append-only log at ~/Library/Logs/tacie/app.log (on iPhone, the app container's Library/Logs) (tacie-dev in a development build) (no question text or card content).
/// Past `maxBytes` the file rolls to app.log.1 (one generation kept), like engine.log.
enum Log {
    private static let url: URL = {
        #if os(macOS)
        let library = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library")
        #else
        // iPhone: the app container's Library (there is no home folder to reach)
        let library = FileManager.default.urls(for: .libraryDirectory, in: .userDomainMask)[0]
        #endif
        let dir = library.appendingPathComponent("Logs/\(Build.logFolder)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("app.log")
    }()
    private static let queue = DispatchQueue(label: "tacie.log")
    private static let stamp = ISO8601DateFormatter()   // used on `queue` only
    static let maxBytes: UInt64 = 5 * 1024 * 1024

    static func info(_ message: String) {
        let now = Date()
        queue.async {
            let line = "\(stamp.string(from: now)) \(message)\n"
            if let h = try? FileHandle(forWritingTo: url) {
                let end = h.seekToEndOfFile()
                if end >= maxBytes {
                    try? h.close()
                    roll()
                    try? Data(line.utf8).write(to: url)
                    return
                }
                h.write(Data(line.utf8))
                try? h.close()
            } else {
                try? Data(line.utf8).write(to: url)
            }
        }
    }

    private static func roll() {
        let old = url.deletingLastPathComponent().appendingPathComponent("app.log.1")
        try? FileManager.default.removeItem(at: old)
        try? FileManager.default.moveItem(at: url, to: old)
    }
}
