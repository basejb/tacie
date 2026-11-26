import Foundation

/// The app's side of `MCPBridge`: the engine, the meeting going on and the meeting folder of the account in use.
@MainActor
final class LiveMCPHost: MCPHost {
    private let engine: EngineProcess
    private let meetingIsRunning: () -> Bool
    private let modelIsReady: () -> Bool
    private let store: () -> MeetingStore

    init(engine: EngineProcess, meetingIsRunning: @escaping () -> Bool, modelIsReady: @escaping () -> Bool,
         store: @escaping () -> MeetingStore) {
        self.engine = engine
        self.meetingIsRunning = meetingIsRunning
        self.modelIsReady = modelIsReady
        self.store = store
    }

    var accountID: String? { AccountSpace.inUse.id }
    var engineGeneration: Int { engine.generation }
    var engineReady: Bool { engine.state == .ready && engine.client != nil }
    var modelReady: Bool { modelIsReady() }
    var meetingRunning: Bool { meetingIsRunning() }

    func engineTool(_ path: String, _ body: [String: Any]) async throws -> Data {
        guard let client = engine.client else { throw EngineError.notRunning }
        return try await client.tool(path, body: body, timeout: 30)
    }

    func meeting(id: String) -> SavedMeeting? { store().load(id: id) }

    func lineName(_ m: SavedMeeting) -> (TranscriptLine) -> String { { m.lineName($0) ?? "" } }
}
