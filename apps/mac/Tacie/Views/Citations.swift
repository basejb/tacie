import AppKit
import SwiftUI

/// The answer line cites files as [1] [2]; those become small raised numbers matching the chips.
enum Citations {
    static let linkScheme = "tacie-cite"

    /// The source number of a `linked` mark's URL.
    static func number(of url: URL) -> Int? {
        guard url.scheme == linkScheme else { return nil }
        return Int(url.absoluteString.dropFirst(linkScheme.count + 1))
    }

    /// The text without its [n] marks, for a copy that has no chips to point at.
    static func unmarked(_ s: String) -> String {
        plain(s).replacingOccurrences(of: #"\s?\[\d\]"#, with: "", options: .regularExpression)
    }

    static func plain(_ s: String) -> String {
        s.replacingOccurrences(of: #"\s*\(?https?://[^\s)]+\)?"#, with: "", options: .regularExpression)
    }

    /// `linked` makes each number a `tacie-cite:<n>` link, for views that open the source a number points at
    /// (the main window's panel reads it with `number(of:)` in its `openURL`).
    static func styled(_ s: String, size: CGFloat, linked: Bool = false) -> AttributedString {
        let text = plain(s)
        var a = (try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(text)
        for n in 1...9 {
            while let r = a.range(of: "[\(n)]") {
                var mark = AttributedString("\u{2009}\(n)")
                mark.font = .system(size: size * 0.6, weight: .semibold)
                mark.foregroundColor = Theme.ink.opacity(0.5)  // an attribute takes a fixed Color, not the per-display ink3
                mark.baselineOffset = size * 0.38
                if linked { mark.link = URL(string: "\(linkScheme):\(n)") }
                a.replaceSubrange(r, with: mark)
            }
        }
        return a
    }

    /// `styled` for AppKit text (a meeting page's selectable answer): bold, italic and code from the inline markdown,
    /// the citation marks raised and small. Its string does not depend on `size`.
    /// The attribute a `selectable(linked:)` mark carries: its source number, for a click to open (`SelectableText.cite`).
    static let citeKey = NSAttributedString.Key("tacieCite")

    static func selectable(_ s: String, size: CGFloat, linked: Bool = false) -> NSAttributedString {
        let text = plain(s)
        let md = (try? AttributedString(markdown: text, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(text)
        let out = NSMutableAttributedString()
        for run in md.runs {
            var attrs: [NSAttributedString.Key: Any] = [:]
            let intent = run.inlinePresentationIntent ?? []
            if intent.contains(.code) {
                attrs[.font] = NSFont.monospacedSystemFont(ofSize: size, weight: .regular)
            } else if intent.contains(.stronglyEmphasized) {
                attrs[.font] = NSFont.systemFont(ofSize: size, weight: .bold)
            } else if intent.contains(.emphasized) {
                attrs[.font] = NSFontManager.shared.convert(NSFont.systemFont(ofSize: size), toHaveTrait: .italicFontMask)
            }
            out.append(NSAttributedString(string: String(md[run.range].characters), attributes: attrs))
        }
        let mark: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: size * 0.6, weight: .semibold),
            .foregroundColor: NSColor(Theme.ink.opacity(0.5)), .baselineOffset: size * 0.38,
        ]
        for n in 1...9 {
            while case let r = (out.string as NSString).range(of: "[\(n)]"), r.location != NSNotFound {
                var attrs = mark
                if linked { attrs[citeKey] = n }
                out.replaceCharacters(in: r, with: NSAttributedString(string: "\u{2009}\(n)", attributes: attrs))
            }
        }
        return out
    }
}
