import Foundation

/// The open edition's step 3: with neither a Tacie server nor OAuth apps, each service is connected by pasting a
/// personal token the person made (docs/41 0절). A pasted token is checked with one request to that service, and only
/// one that works is saved. The check goes through URLSession only and costs nothing.
/// The GitHub card's Connect: through Tacie's GitHub app (official), or a pasted token (open edition).
enum GitHubConnectOffer {
    static func canConnect(clientID: String, pasted: Bool) -> Bool { !clientID.isEmpty || pasted }
}

enum PastedTokens {
    /// A source build connects by pasted tokens; a build with the official values keeps its "연결" (OAuth) buttons.
    static let offered = Build.isOpen

    /// Whether this service connects by a pasted token (#56): always in a source build; Notion, Linear,
    /// Jira·Confluence and Asana also without a Tacie session, because their one-click connect hands the code over on
    /// Tacie's server with that session (apps/server notion.js, linear.js, atlassian.js, asana.js answer 401
    /// signed_out). GitHub, Google Drive and Dropbox connect without it.
    static let needsSession: Set<ConnectionsScene.Source> = [.notion, .linear, .atlassian, .asana]
    static func pasted(_ s: ConnectionsScene.Source, signedIn: Bool, hasAccount: Bool) -> Bool {
        guard hasAccount else { return true }
        return needsSession.contains(s) && !signedIn
    }
    @MainActor static func pasted(_ s: ConnectionsScene.Source) -> Bool {
        pasted(s, signedIn: TacieAccount.shared.signedIn, hasAccount: Build.hasAccount)
    }

    struct Found: Equatable {
        var name: String?
        var id: String?
        var cloud: String?
        var siteURL: String?
    }

    enum Check: Equatable {
        case ok(Found)
        /// 401 or 403: the service refused the token.
        case rejected
        /// Offline, 5xx, anything else: not known now (nothing is saved).
        case unknown
        /// Jira·Confluence: no Atlassian site at that address.
        case noSite
    }

    /// Where the person makes the token. GitHub's page opens with the permissions already chosen (template URL).
    static func makeURL(_ s: ConnectionsScene.Source) -> URL {
        switch s {
        case .github:
            var c = URLComponents(string: "https://github.com/settings/personal-access-tokens/new")!
            c.queryItems = [
                URLQueryItem(name: "name", value: "Tacie"),
                URLQueryItem(name: "description", value: "Tacie reads code, issues and PRs"),
                URLQueryItem(name: "contents", value: "read"),
                URLQueryItem(name: "issues", value: "read"),
                URLQueryItem(name: "pull_requests", value: "read"),
                URLQueryItem(name: "metadata", value: "read"),
            ]
            return c.url!
        case .notion: return URL(string: "https://www.notion.so/profile/integrations")!
        case .linear: return URL(string: "https://linear.app/settings/account/security")!
        case .asana: return URL(string: "https://app.asana.com/0/my-apps")!
        case .atlassian: return URL(string: "https://id.atlassian.com/manage-profile/security/api-tokens")!
        case .gdrive, .dropbox, .notes: return URL(string: "https://tacie.app")!  // never shown: no token for these
        }
    }

    /// "myteam", "myteam.atlassian.net", "https://myteam.atlassian.net/" → "https://myteam.atlassian.net".
    static func siteURL(_ typed: String) -> String? {
        var host = typed.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        for scheme in ["https://", "http://"] where host.hasPrefix(scheme) { host.removeFirst(scheme.count) }
        // an address copied from the browser carries a page after the host
        if let slash = host.firstIndex(of: "/") { host = String(host[..<slash]) }
        guard !host.isEmpty, !host.contains("/"), !host.contains(" ") else { return nil }
        if !host.contains(".") { host += ".atlassian.net" }
        return "https://" + host
    }

    static func check(_ s: ConnectionsScene.Source, token: String, email: String? = nil, site: String? = nil,
                      get: AIChecks.Get = AIChecks.live) async -> Check {
        let token = token.trimmingCharacters(in: .whitespacesAndNewlines)
        switch s {
        case .github:
            var r = request("https://api.github.com/user", bearer: token)
            r.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
            return await ask(r, get) { j in Found(name: j["login"] as? String) }
        case .notion:
            var r = request("https://api.notion.com/v1/users/me", bearer: token)
            r.setValue("2022-06-28", forHTTPHeaderField: "Notion-Version")
            return await ask(r, get) { j in Found(name: (j["bot"] as? [String: Any])?["workspace_name"] as? String) }
        case .linear:
            var r = URLRequest(url: URL(string: "https://api.linear.app/graphql")!, timeoutInterval: 10)
            r.httpMethod = "POST"
            r.setValue(token.hasPrefix("lin_api_") ? token : "Bearer \(token)", forHTTPHeaderField: "Authorization")
            r.setValue("application/json", forHTTPHeaderField: "Content-Type")
            r.httpBody = Data(#"{"query":"{ organization { id name } }"}"#.utf8)
            return await ask(r, get) { j in
                guard let org = (j["data"] as? [String: Any])?["organization"] as? [String: Any] else { return nil }
                return Found(name: org["name"] as? String, id: org["id"] as? String)
            }
        case .asana:
            let r = request("https://app.asana.com/api/1.0/users/me?opt_fields=gid,name", bearer: token)
            return await ask(r, get) { j in
                guard let d = j["data"] as? [String: Any], let gid = d["gid"] as? String else { return nil }
                return Found(name: d["name"] as? String, id: gid)
            }
        case .atlassian:
            guard let base = site.flatMap(siteURL), let email = email?.trimmingCharacters(in: .whitespacesAndNewlines),
                  !email.isEmpty else { return .noSite }
            let tenant = URLRequest(url: URL(string: base + "/_edge/tenant_info")!, timeoutInterval: 10)
            guard let (data, http) = try? await get(tenant) else { return .unknown }
            guard http.statusCode == 200,
                  let cloud = (try? JSONSerialization.jsonObject(with: data) as? [String: Any])?["cloudId"] as? String,
                  !cloud.isEmpty else { return .noSite }
            var r = URLRequest(url: URL(string: base + "/rest/api/3/myself")!, timeoutInterval: 10)
            r.setValue("Basic " + Data("\(email):\(token)".utf8).base64EncodedString(), forHTTPHeaderField: "Authorization")
            r.setValue("application/json", forHTTPHeaderField: "Accept")
            let host = String(base.dropFirst("https://".count))
            let found = Found(name: host, cloud: cloud, siteURL: base)
            // Jira first; a site without Jira (or a person without access to it) may still have Confluence.
            // 401 is the token; 403 or 404 on one product says nothing about it.
            guard let (_, jira) = try? await get(r) else { return .unknown }
            if jira.statusCode == 200 { return .ok(found) }
            if jira.statusCode == 401 { return .rejected }
            var w = URLRequest(url: URL(string: base + "/wiki/api/v2/spaces?limit=1")!, timeoutInterval: 10)
            w.setValue(r.value(forHTTPHeaderField: "Authorization"), forHTTPHeaderField: "Authorization")
            w.setValue("application/json", forHTTPHeaderField: "Accept")
            guard let (_, wiki) = try? await get(w) else { return .unknown }
            switch wiki.statusCode {
            case 200: return .ok(found)
            case 401: return .rejected
            default: return jira.statusCode == 403 && wiki.statusCode == 403 ? .rejected : .unknown
            }
        case .gdrive, .dropbox, .notes:
            return .unknown
        }
    }

    private static func request(_ url: String, bearer token: String) -> URLRequest {
        var r = URLRequest(url: URL(string: url)!, timeoutInterval: 10)
        r.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        return r
    }

    /// 200 with what `read` finds: ok. 200 without it (Linear answers a bad key with 200 and `errors`), 401, 403:
    /// rejected. Anything else: unknown.
    private static func ask(_ r: URLRequest, _ get: AIChecks.Get, read: ([String: Any]) -> Found?) async -> Check {
        guard let (data, http) = try? await get(r) else { return .unknown }
        // Linear answers a refused key with 400 and an authentication code in the body
        if let errors = (try? JSONSerialization.jsonObject(with: data) as? [String: Any])?["errors"] as? [[String: Any]],
           errors.contains(where: { ["AUTHENTICATION_ERROR", "FORBIDDEN"].contains(($0["extensions"] as? [String: Any])?["code"] as? String) }) {
            return .rejected
        }
        switch http.statusCode {
        case 200:
            let json = (try? JSONSerialization.jsonObject(with: data) as? [String: Any]) ?? [:]
            return read(json).map { .ok($0) } ?? .rejected
        case 401, 403: return .rejected
        default: return .unknown
        }
    }
}
