import Foundation
import Sparkle

/// The release notes page holds every version (scripts/release-notes-page.py); `?since=<installed version>` makes its
/// script show each version this Mac skipped. Both places that show the page pass it: our What's changed link and
/// Sparkle's own window (Check for updates), whose item gets the link rewritten and which runs the script
/// (SUEnableJavaScript).
///
/// From 0.1.36 each version has a Korean and an English page, Tacie-<version>.ko.html and .en.html, both linked in the
/// appcast with xml:lang. Sparkle picks by the Mac's languages; Tacie shows the one in its own language instead, so the
/// notes match the app. Older versions have one Korean page, Tacie-<version>.html, kept as it is.
enum UpdateNotes {
    /// "en" when Tacie runs in English, "ko" otherwise.
    static var language: String { Bundle.main.preferredLocalizations.first == "en" ? "en" : "ko" }

    /// The release notes page of `version` on the update server, in `language`.
    static func page(version: String, language: String = language) -> URL? {
        URL(string: BuildValues.modelsBase.absoluteString + "/updates/Tacie-\(version).\(language).html")
    }

    /// `url` switched to the page in `language` when it is one of the two per-language pages; any other link unchanged.
    static func localized(_ url: URL?, language: String = language) -> URL? {
        guard let url, var parts = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return url }
        let name = url.lastPathComponent
        for other in ["ko", "en"] where name.hasSuffix(".\(other).html") {
            guard other != language else { return url }
            let base = name.dropLast(".\(other).html".count)
            parts.path = url.deletingLastPathComponent().path + "/" + base + ".\(language).html"
            return parts.url ?? url
        }
        return url
    }

    /// `url` with `since=<installed>` added, unless it already has one.
    static func since(_ url: URL?, installed: String?) -> URL? {
        guard let url, let installed,
              var parts = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return url }
        var items = parts.queryItems ?? []
        if items.contains(where: { $0.name == "since" }) { return url }
        items.append(URLQueryItem(name: "since", value: installed))
        parts.queryItems = items
        return parts.url ?? url
    }

    /// The same appcast item with its release notes link in Tacie's language and `since=<installed>` on it, or nil if it
    /// has no link or cannot be rebuilt (the caller then keeps Sparkle's own item).
    static func item(_ item: SUAppcastItem, installed: String?, language: String = language) -> SUAppcastItem? {
        var dict = item.propertiesDictionary
        let key = "sparkle:releaseNotesLink"
        // Sparkle keeps the element as its attributes plus "content" (the link), or, from older parsers, the bare link
        var element = dict[key] as? [String: Any] ?? [:]
        guard let link = element["content"] as? String ?? dict[key] as? String, let url = URL(string: link),
              let with = since(localized(url, language: language), installed: installed), with != url else { return nil }
        if dict[key] is [String: Any] {
            element["content"] = with.absoluteString
            dict[key] = element
        } else {
            dict[key] = with.absoluteString
        }
        // Sparkle marks this initializer deprecated for appcasts with version-dependent elements (phased rollout,
        // minimum autoupdate version, informational updates); ours have none of them.
        return SUAppcastItem(dictionary: dict, relativeTo: nil, failureReason: nil)
    }
}
