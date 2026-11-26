import XCTest

/// The String Catalogs (docs/28): every Korean key has its English, with the same format arguments, and counts
/// ("질문 %lld개") have English singular and plural forms.
final class LocalizationCatalogTests: XCTestCase {
    private func catalog(_ name: String) throws -> [String: Any] {
        let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .appendingPathComponent("../Tacie/\(name)").standardizedFileURL
        let json = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any]
        XCTAssertEqual(json?["sourceLanguage"] as? String, "ko")
        return try XCTUnwrap(json?["strings"] as? [String: Any])
    }

    private func hasHangul(_ s: String) -> Bool {
        s.unicodeScalars.contains { (0xAC00...0xD7A3).contains($0.value) }
    }

    /// The format arguments in order of their positions: "%2$@ %1$lld" → ["lld", "@"].
    private func arguments(_ s: String) -> [String] {
        let rx = try! NSRegularExpression(pattern: #"%(?:(\d)\$)?(lld|ld|d|@|lf|f|\.\d+f|\.\d+lf)"#)
        var out: [(Int, String)] = []
        var next = 1
        for m in rx.matches(in: s, range: NSRange(s.startIndex..., in: s)) {
            let type = String(s[Range(m.range(at: 2), in: s)!])
            if let r = Range(m.range(at: 1), in: s) { out.append((Int(s[r])!, type)) } else { out.append((next, type)); next += 1 }
        }
        return out.sorted { $0.0 < $1.0 }.map(\.1)
    }

    /// English values of one entry: a plain value, or the plural forms keyed "one"/"other".
    private func english(_ entry: [String: Any]) -> [String: String]? {
        guard let en = (entry["localizations"] as? [String: Any])?["en"] as? [String: Any] else { return nil }
        if let value = (en["stringUnit"] as? [String: Any])?["value"] as? String { return ["": value] }
        if let plural = (en["variations"] as? [String: Any])?["plural"] as? [String: Any] {
            return plural.compactMapValues { (($0 as? [String: Any])?["stringUnit"] as? [String: Any])?["value"] as? String }
        }
        return nil
    }

    func testEveryKoreanKeyHasEnglish() throws {
        let strings = try catalog("Localizable.xcstrings")
        XCTAssertGreaterThan(strings.count, 500)
        var missing: [String] = []
        for (key, value) in strings {
            let entry = value as? [String: Any] ?? [:]
            if entry["shouldTranslate"] as? Bool == false { continue }
            // a key without Korean (a symbol, a product name, an English key with a Korean default) may fall back
            let source = (((entry["localizations"] as? [String: Any])?["ko"] as? [String: Any])?["stringUnit"]
                as? [String: Any])?["value"] as? String ?? key
            guard hasHangul(source) else { continue }
            guard let en = english(entry), !en.isEmpty else { missing.append(key); continue }
            for (form, text) in en {
                XCTAssertFalse(text.isEmpty, "\(key) [\(form)]")
                XCTAssertFalse(hasHangul(text), "Korean left in English: \(key) [\(form)] = \(text)")
                // a plural form may leave out the count ("one" = "a question"), never add or retype one
                let want = arguments(source), got = arguments(text)
                if form == "one" { XCTAssertTrue(got.allSatisfy(want.contains), "\(key) [one] = \(text)") }
                else { XCTAssertEqual(got, want, "arguments differ: \(key) [\(form)] = \(text)") }
            }
            if source.contains("%lld개") || source.contains("%lld명") {
                XCTAssertNotNil(en["one"], "count without singular English: \(key)")
                XCTAssertNotNil(en["other"], "count without plural English: \(key)")
            }
        }
        XCTAssertEqual(missing, [], "Korean keys without English")
    }

    func testPermissionTextsHaveEnglish() throws {
        let strings = try catalog("InfoPlist.xcstrings")
        for key in ["NSMicrophoneUsageDescription", "NSAudioCaptureUsageDescription"] {
            let entry = try XCTUnwrap(strings[key] as? [String: Any], key)
            let en = try XCTUnwrap(english(entry)?[""], key)
            XCTAssertFalse(en.isEmpty || hasHangul(en), key)
        }
    }
}
