import Foundation

struct DeviceCode: Equatable {
    let deviceCode: String
    let userCode: String
    let verificationURI: URL
    let expiresIn: Int
    let interval: Int
}

enum DeviceFlowError: Error, Equatable { case expired, denied, unavailable, network }

/// GitHub's OAuth device flow. Public client id only; there is no client secret.
struct GitHubDevice {
    /// The OAuth App's Client ID (public; official builds only, Official.xcconfig). Empty hides the connect button.
    static let clientID = BuildValues.gitHubClientID ?? ""
    static let scope = "repo read:org"
    static var approvalPage: URL? {
        clientID.isEmpty ? nil : URL(string: "https://github.com/settings/connections/applications/\(clientID)")
    }

    private struct BadStatus: Error {}

    /// Returns the body of a 2xx answer; any other status throws.
    var send: (URLRequest) async throws -> Data = { req in
        let (data, resp) = try await URLSession.shared.data(for: req)
        if let http = resp as? HTTPURLResponse, !(200..<300).contains(http.statusCode) { throw BadStatus() }
        return data
    }
    var sleep: (Int) async throws -> Void = { try await Task.sleep(for: .seconds($0)) }

    func start() async throws -> DeviceCode {
        let obj = try await post("https://github.com/login/device/code",
                                 ["client_id": Self.clientID, "scope": Self.scope])
        guard obj["error"] == nil,
              let device = obj["device_code"] as? String, let user = obj["user_code"] as? String,
              let uri = (obj["verification_uri"] as? String).flatMap(URL.init(string:)),
              let expires = obj["expires_in"] as? Int else { throw DeviceFlowError.unavailable }
        return DeviceCode(deviceCode: device, userCode: user, verificationURI: uri,
                          expiresIn: expires, interval: max(1, obj["interval"] as? Int ?? 5))  // 0 would poll without a pause
    }

    func token(for code: DeviceCode) async throws -> String {
        var interval = code.interval
        var slept = 0
        while true {
            if slept + interval > code.expiresIn { throw DeviceFlowError.expired }
            try await wait(interval)
            slept += interval
            let obj = try await post("https://github.com/login/oauth/access_token",
                                     ["client_id": Self.clientID, "device_code": code.deviceCode,
                                      "grant_type": "urn:ietf:params:oauth:grant-type:device_code"])
            if let token = obj["access_token"] as? String, !token.isEmpty {
                // whether GitHub hands out an expiring token; only the presence and the lifetime, never a value
                let refresh = (obj["refresh_token"] as? String).map { !$0.isEmpty } ?? false
                let expires = (obj["expires_in"] as? Int).map(String.init) ?? "none"
                Log.info("github token: refresh_token \(refresh ? "yes" : "no"), expires_in \(expires)")
                return token
            }
            switch obj["error"] as? String {
            case "authorization_pending": continue
            case "slow_down": interval = max(1, obj["interval"] as? Int ?? interval + 5)
            case "expired_token": throw DeviceFlowError.expired
            case "access_denied": throw DeviceFlowError.denied
            default: throw DeviceFlowError.unavailable
            }
        }
    }

    private func wait(_ seconds: Int) async throws {
        do { try await sleep(seconds) } catch { throw Self.mapped(error) }
        if Task.isCancelled { throw CancellationError() }
    }

    private func post(_ url: String, _ fields: [String: String]) async throws -> [String: Any] {
        if Task.isCancelled { throw CancellationError() }
        var req = URLRequest(url: URL(string: url)!)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Accept")
        req.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
        req.httpBody = Data(Self.form(fields).utf8)
        let data: Data
        do { data = try await send(req) } catch { throw Self.mapped(error) }
        guard let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw DeviceFlowError.unavailable
        }
        return obj
    }

    private static func mapped(_ error: Error) -> Error {
        if error is CancellationError || (error as? URLError)?.code == .cancelled { return CancellationError() }
        if error is DeviceFlowError { return error }
        if error is URLError { return DeviceFlowError.network }
        return DeviceFlowError.unavailable
    }

    private static func form(_ fields: [String: String]) -> String {
        var allowed = CharacterSet.urlQueryAllowed
        allowed.remove(charactersIn: "&+=")
        return fields.sorted { $0.key < $1.key }.map { key, value in
            "\(key)=\(value.addingPercentEncoding(withAllowedCharacters: allowed) ?? "")"
        }.joined(separator: "&")
    }
}
