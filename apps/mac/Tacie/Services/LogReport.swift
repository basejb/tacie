import Foundation

/// The problem report: the app's own logs, zipped, without anything that was said or typed.
/// Plain Foundation so the unit tests (no app host) can compile it.
enum LogReport {
    static let email = "support@tacie.app"  // 2026-11-11 forwarded by Cloudflare Email Routing
    /// The open edition has no support address: a report goes to the public repository as a bug issue.
    static let issueURL = URL(string: "https://github.com/basejb/tacie/issues/new?template=bug.yml")!

    static var logs: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/\(Build.logFolder)")
    }

    private static let files = ["app.log", "app.log.1", "engine.log", "engine.log.1"]
    private static let engineTextPrefixes = ["[heard]", "[ask]", "[answer]"]
    // older app.log lines from the debug commands, which hold typed text
    private static let appTextMarkers = ["debug: ask ", "debug: search ", "debug: slack "]

    /// Drops the lines that can hold meeting words or typed questions.
    static func filtered(_ log: String) -> String {
        // split on "\n" only: Swift's Character treats "\r\n" as one character, so split(separator:) would miss it
        log.components(separatedBy: "\n").filter { raw in
            let line = raw.hasSuffix("\r") ? String(raw.dropLast()) : raw
            return !engineTextPrefixes.contains { line.hasPrefix($0) } && !appTextMarkers.contains { line.contains($0) }
        }.joined(separator: "\n")
    }

    /// Copies the log files, filtered, into `dir`. Nothing else; a missing file is skipped.
    static func stage(logs: URL, into dir: URL) throws -> [URL] {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        var staged: [URL] = []
        for name in files {
            guard let data = try? Data(contentsOf: logs.appendingPathComponent(name)) else { continue }
            let text = filtered(String(decoding: data, as: UTF8.self))
            let to = dir.appendingPathComponent(name)
            try text.write(to: to, atomically: true, encoding: .utf8)
            staged.append(to)
        }
        return staged
    }

    /// logs/reports/Tacie-logs-yyyyMMdd-HHmm.zip
    static func make(now: Date = Date()) throws -> URL {
        let fm = FileManager.default
        let reports = logs.appendingPathComponent("reports")
        try fm.createDirectory(at: reports, withIntermediateDirectories: true)
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyyMMdd-HHmm"
        let name = "Tacie-logs-\(f.string(from: now))"
        let tmp = fm.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? fm.removeItem(at: tmp) }
        let folder = tmp.appendingPathComponent(name)
        _ = try stage(logs: logs, into: folder)
        let zip = reports.appendingPathComponent("\(name).zip")
        try? fm.removeItem(at: zip)
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
        p.arguments = ["-c", "-k", "--keepParent", folder.path, zip.path]
        try p.run()
        p.waitUntilExit()
        guard p.terminationStatus == 0 else { throw CocoaError(.fileWriteUnknown) }
        return zip
    }
}
