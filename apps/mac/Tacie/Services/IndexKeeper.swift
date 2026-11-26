import Foundation
import Observation

/// Keeps the search material current (engine/crates/tacie-engine/src/refresh.rs): the menu's "Sync now", and on its own
/// while no meeting is on: every 30 minutes it asks the engine whether the branch or GitHub changed and syncs if so, or
/// once the material is older than 20 hours. The engine answers from the old index until the new one is swapped in.
@Observable
@MainActor
final class IndexKeeper {
    private(set) var builtAt: Date?
    private(set) var commit: String?
    private(set) var step: String?  // while a sync runs, what it is doing ("검색 준비 중")
    private(set) var rawStep: String?  // the engine's name of it ("repo", "sources", "notion", "embed", "swap")
    private(set) var progress: (done: Int, total: Int)?  // while embedding: chunks done of those to embed this run
    private(set) var problem: String?  // the last refresh failed; the old index is still in use
    private(set) var info: IndexState?  // the last GET /index (연결, 개발자)
    private(set) var problemKind: String?  // errorKind of the last failed refresh
    private var problemReason = "other"  // SyncRules.githubReason of it, for app.log
    private(set) var partial: String?  // the last refresh kept a saved copy of a source, or left GitHub out
    private(set) var switchingTo: String?  // while a sync moves the search data to another repo: that repo
    /// The Notion connection (docs/13): the engine gets it with the GitHub login.
    let notion = NotionLink()
    /// The Linear connection (docs/18): the app is the only place that refreshes its token.
    let linear = LinearLink()
    /// The Google Drive connection (docs/19): the app is the only place that refreshes its one-hour token.
    let gdrive = GDriveLink()
    /// The Jira · Confluence connection (docs/31): the app is the only place that refreshes its one-hour token.
    let atlassian = AtlassianLink()
    /// The Asana connection (docs/32): the app is the only place that refreshes its one-hour token.
    let asana = AsanaLink()
    /// The Dropbox connection (docs/33): the app is the only place that refreshes its four-hour token.
    let dropbox = DropboxLink()
    /// Something to answer from is connected: GitHub, or Notion, Linear, Google Drive, Jira · Confluence, Asana,
    /// Dropbox or a note folder alone.
    var anySource: Bool {
        connected || notion.connected || linear.connected || gdrive.connected || atlassian.connected || asana.connected
            || dropbox.connected || notesOn
    }
    /// A note folder is chosen (docs/34): the 30-minute check also asks the engine to read changed notes again.
    var notesOn: Bool { (notesInfo?.chosen ?? 0) > 0 }
    /// GitHub refused the login on the last sync (revoked, or the repo is out of reach): connect again (설정 → 연결).
    var githubRefused: Bool { problemKind == "key" && step == nil }
    /// The connections that were refused and need 다시 연결 in 설정 → 연결, in the page's order. Searches go on from
    /// what was fetched, so without this line nothing would tell the user that new pages stopped coming in.
    var lostNames: [String] {
        var names: [String] = []
        if githubRefused { names.append("GitHub") }
        if notion.connected && notion.lost { names.append(String(localized: "노션")) }
        // the engine's lost_since counts too, as on the cards (it can know before the app does)
        if linear.connected && (linear.lost || linearInfo?.lostSince != nil) { names.append("Linear") }
        if gdrive.connected && (gdrive.lost || gdriveInfo?.lostSince != nil) { names.append("Google Drive") }
        if atlassian.connected && (atlassian.lost || atlassianInfo?.lostSince != nil) { names.append("Jira · Confluence") }
        if asana.connected && (asana.lost || asanaInfo?.lostSince != nil) { names.append("Asana") }
        if dropbox.connected && (dropbox.lost || dropboxInfo?.lostSince != nil) { names.append("Dropbox") }
        return names
    }
    /// The repository (or Notion) whose first sync is running: nothing can be searched until it ends (docs/11 11절).
    var firstSync: String? {
        // a first sync stays first while its parts go into search (built_at comes only when it ends)
        guard step != nil, (info?.chunks ?? 0) == 0 || builtAt == nil || switchingTo != nil else { return nil }
        return switchingTo ?? info?.repo ?? (notion.connected ? String(localized: "노션") : linear.connected ? "Linear" : gdrive.connected ? "Google Drive"
            : atlassian.connected ? "Jira · Confluence" : asana.connected ? "Asana" : dropbox.connected ? "Dropbox" : nil)
    }
    /// How far the whole sync is, 0–1 (the main window's bar): each step owns a share, and the steps that report
    /// progress (Notion's pages, embedding) fill theirs as they go. Never moves back within a sync.
    var syncFraction: Double? {
        guard step != nil else { return nil }
        // Jira · Confluence reports how many so far, not toward a total: its share fills when its step ends
        let p = rawStep == "atlassian" ? nil : progress.map { $0.total > 0 ? min(1, Double($0.done) / Double($0.total)) : 0 }
        return SyncRules.fraction(step: rawStep, progress: p, notion: notion.connected, linear: linear.connected,
                                  gdrive: gdrive.connected, atlassian: atlassian.connected, asana: asana.connected,
                                  dropbox: dropbox.connected)
    }

    /// GitHub, 노션 and 검색 준비 as rows while a sync with both sources runs (nil otherwise).
    var syncRows: [SyncRules.Row]? {
        guard step != nil else { return nil }
        return SyncRules.rows(step: rawStep, progress: progress, repo: switchingTo ?? info?.repo,
                              notion: notion.connected, linear: linear.connected, gdrive: gdrive.connected,
                              atlassian: atlassian.connected, asana: asana.connected, dropbox: dropbox.connected)
    }

    /// What the running sync cannot answer from yet, for the line under its progress (docs/13): everything on the first
    /// sync, Notion's pages on Notion's first, nothing new otherwise (the old search data answers meanwhile).
    var syncNote: String? {
        guard step != nil else { return nil }
        if info?.refresh.partial == true { return String(localized: "받은 만큼 찾고 있어요. 끝나면 전부 찾아요.") }
        if firstSync != nil { return String(localized: "끝나면 회의 질문에 답해요. 그 전 질문은 적어 뒀다가 찾아요.") }
        if notion.connected, info?.notion?.pages == nil { return String(localized: "끝나면 노션에서도 찾아요. 그동안은 지난 자료로 답해요.") }
        if linear.connected, linearInfo?.issues == nil { return String(localized: "끝나면 Linear에서도 찾아요. 그동안은 지난 자료로 답해요.") }
        if gdrive.connected, gdriveInfo?.files == nil { return String(localized: "끝나면 Google Drive에서도 찾아요. 그동안은 지난 자료로 답해요.") }
        if gdrive.connected, gdriveInfo?.initialDone == false { return String(localized: "Google Drive 문서는 받은 만큼 찾고, 나머지는 이어서 받아요.") }
        if atlassian.connected, atlassianInfo?.issues == nil, atlassianInfo?.pages == nil {
            return String(localized: "끝나면 Jira · Confluence에서도 찾아요. 그동안은 지난 자료로 답해요.")
        }
        if atlassian.connected, atlassianInfo?.initialDone == false {
            return String(localized: "받는 동안에도 이미 받은 Jira · Confluence 자료에서 찾아요.")
        }
        if asana.connected, asanaInfo?.tasks == nil {
            return String(localized: "끝나면 Asana에서도 찾아요. 그동안은 지난 자료로 답해요.")
        }
        if asana.connected, asanaInfo?.initialDone == false {
            return String(localized: "받는 동안에도 이미 받은 Asana task에서 찾아요.")
        }
        if dropbox.connected, dropboxInfo?.files == nil {
            return String(localized: "끝나면 Dropbox에서도 찾아요. 그동안은 지난 자료로 답해요.")
        }
        if dropbox.connected, dropboxInfo?.initialDone == false {
            return String(localized: "받는 동안에도 이미 받은 Dropbox 파일에서 찾아요.")
        }
        if let to = switchingTo { return String(localized: "끝나면 \(to)에서 찾아요. 그동안은 지난 자료로 답해요.") }
        return String(localized: "그동안은 지난 자료로 답해요.")
    }
    /// "검색 준비 중 · 42%": the running sync's step and how far the whole sync is (the same number as the main
    /// window's bar, everywhere).
    var stepLine: String? {
        guard let step else { return nil }
        guard let f = syncFraction else { return step }
        return "\(step) · \(Int(f * 100))%"
    }
    /// The engine has a Jev key or the relay (2-C, CardsGate). True until the engine has said otherwise.
    private(set) var jevAvailable = true
    private(set) var connected = false  // the engine holds a working GitHub login
    private(set) var cleared = false  // the search data was deleted: nothing syncs until the user picks a repo
    /// A meeting is on: no automatic refresh then (embedding shares the GPU with the questions).
    var busy: () -> Bool = { false }
    /// Which GitHub login the engine gets (AppDelegate decides by build and settings).
    var credentials: () -> (mode: String, token: String?) = { ("gh", nil) }
    /// The AI keys the engine gets (from the keychain, AIKeys.saved).
    /// The user's own keys (a development build without a sign-in).
    /// With the Tacie AI relay, the session stands in for both keys (docs/14).
    var aiKeys: () -> [AIKeys.Name: String?] = {
        if Relay.on {
            let session = TacieAccount.shared.token
            return [.openrouter: session, .typesafe: session]
        }
        return AIMode.current.keys  // the open edition's AI mode may hold keys back (2-D)
    }
    private let engine: EngineProcess
    private var timer: Timer?
    private var onReady: Task<Void, Never>?
    /// The past meetings the engine searches (docs/30), set by the app from its library.
    var meetings: () -> [MeetingDigest] = { [] }
    /// What the engine running now was last given: an unchanged list is not sent again.
    private var sentMeetings: [MeetingDigest]?
    private var meetingsSoon: Task<Void, Never>?
    /// Counts engine starts and stops, so a send that outlived its engine does not count as given to the next.
    private var engineRun = 0

    init(engine: EngineProcess) {
        self.engine = engine
        notion.push = { [weak self] c in await self?.pushNotion(c) }
        linear.push = { [weak self] c in await self?.pushLinear(c) }
        linear.onConnected = { Analytics.capture(.linearConnected) }
        gdrive.push = { [weak self] c in await self?.pushGDrive(c) }
        gdrive.onConnected = { Analytics.capture(.gdriveConnected) }
        atlassian.push = { [weak self] c in await self?.pushAtlassian(c) }
        atlassian.onConnected = { Analytics.capture(.atlassianConnected) }
        asana.push = { [weak self] c in await self?.pushAsana(c) }
        asana.onConnected = { Analytics.capture(.asanaConnected) }
        dropbox.push = { [weak self] c in await self?.pushDropbox(c) }
        dropbox.onConnected = { Analytics.capture(.dropboxConnected(team: $0.team, paperLegacy: $0.paperLegacy)) }
    }

    /// Before the account changes (AccountSpace): connections being made stop and token renewals in flight end, so
    /// nothing lands in the next account's keychain items.
    func settle() async {
        await notion.settle()
        await linear.settle()
        await gdrive.settle()
        await atlassian.settle()
        await asana.settle()
        await dropbox.settle()
    }

    /// After the account changed: its connections, and nothing of the last account's search data (the new engine
    /// reports its own once it is ready).
    func reload() {
        notion.reload()
        linear.reload()
        gdrive.reload()
        atlassian.reload()
        asana.reload()
        dropbox.reload()
        builtAt = nil
        commit = nil
        step = nil
        rawStep = nil
        progress = nil
        problem = nil
        problemKind = nil
        problemReason = "other"
        partial = nil
        switchingTo = nil
        info = nil
        connected = false
        cleared = false
        waitingForModel = nil
        sentMeetings = nil
    }

    /// Debug captures: the sync line (and the scene's running light) as while a sync runs, without one; nil ends it.
    func debugSyncing(_ step: String?) { self.step = step }

    /// "동기화하는 중 · 검색 준비 중 42%" while a sync runs.
    var syncLine: String? {
        stepLine.map { String(localized: "동기화하는 중 · \($0)") }
    }

    /// 일반 설정 "자동 동기화" (on unless turned off).
    static var autoSyncOn: Bool { UserDefaults.standard.object(forKey: "autoSync") as? Bool ?? true }

    func start() {
        timer = Timer.scheduledTimer(withTimeInterval: 1800, repeats: true) { [weak self] _ in
            Task { @MainActor in
                await self?.linear.ensureFresh()
                await self?.gdrive.ensureFresh()
                await self?.atlassian.retryUnreadable()
                await self?.atlassian.ensureFresh()
                await self?.asana.retryUnreadable()
                await self?.asana.ensureFresh()
                await self?.dropbox.retryUnreadable()
                await self?.dropbox.ensureFresh()
                await self?.check(auto: true)
            }
        }
        watchEngine()
        Task {
            // a sync waits a minute after launch, out of the way of the first questions
            try? await Task.sleep(for: .seconds(60))
            await check(auto: true)
        }
    }

    /// Every time the engine becomes ready (also after a restart, which forgets the credentials): hand it the
    /// GitHub login first, then read how fresh the material is. The AI keys arrived before `.ready` (`prepare`).
    private func watchEngine() {
        let state = withObservationTracking { engine.state } onChange: { [weak self] in
            Task { @MainActor in
                self?.engineChanged()
                self?.watchEngine()
            }
        }
        if state == .ready, onReady == nil { engineChanged() }
    }

    private func engineChanged() {
        engineRun += 1
        onReady?.cancel()
        onReady = nil
        guard engine.state == .ready else {
            sentMeetings = nil  // a new engine is given the list again
            info = nil
            step = nil
            progress = nil
            connected = false  // a restarted engine holds no login until the next push
            return
        }
        onReady = Task {
            await pushCredentials()
            await check(auto: false)
        }
    }

    /// After a key is saved or removed, to the running engine. A starting engine gets them through
    /// `EngineProcess.prepare` before it reports ready (AppDelegate wires `pushAIKeys(to:)` there).
    func pushAIKeys() async {
        guard let client = engine.client else { return }
        await pushAIKeys(to: client)
    }

    /// The engine holds AI keys in memory only.
    func pushAIKeys(to client: EngineClient) async {
        // a meeting under way closes a Soniox connection the new choice would not open (MeetingModel.aiChoiceChanged)
        NotificationCenter.default.post(name: AIChoice.changed, object: nil)
        // Tacie's own notes server (issue #54) runs from here on: it loads the model, unloads it when idle and loads it
        // again on the next request, so the engine can reach it whenever notes or the main window's AI ask
        if Build.isOpen, AIMode.saved() == .local, let saved = LocalServer.saved(), saved.name == BundledLLM.name,
           let running = await BundledLLM.shared.server(), running != saved {
            LocalServer.save(running)
        }
        do {
            _ = try await client.setAIKeys(aiKeys(), chat: AIAddress.current, stt: SttProviderChoice.current)
            // whether meeting cards can run (2-C): the engine names its Jev provider, or nil without a key
            if let c = try? await client.config() { jevAvailable = c.models.jev != nil }
        } catch {
            Log.info("ai keys not accepted: \((error as? EngineError)?.kind ?? "other")")
        }
    }

    func pushCredentials() async {
        guard let client = engine.client else { return }
        // `--no-connections` (landing captures): the engine answers from its own index only, so no saved connection
        // starts a sync of the user's real GitHub, Notion, Linear or Drive into it, and no token gets refreshed
        if LaunchArgs.has("--no-connections") {
            await pushMeetings()  // only with `--push-meetings` (see pushMeetingsOnce)
            if let s = try? await client.indexState() { apply(s) }
            return
        }
        let c = credentials()
        do {
            connected = try await client.setGitHub(mode: c.mode, token: c.token)
            // which login went over (never the token): "none" in a product build means the keychain gave nothing back
            Log.info("github login handed over: \(c.mode), \(connected ? "connected" : "not connected")")
        } catch {
            connected = false
            Log.info("github login not accepted: \((error as? EngineError)?.kind ?? "other")")
        }
        await pushNotion(notion.saved)
        // the saved Linear connection goes over right away (the engine keeps muted teams on disk anyway), then the
        // token is refreshed if it ends within the hour
        await pushLinear(linear.saved, follow: false)
        Task { await linear.ensureFresh() }
        // Drive's token lasts an hour: refresh first if it is (nearly) over, then hand it over
        await gdrive.ensureFresh()
        await pushGDrive(gdrive.saved, follow: false)
        // Jira · Confluence's token lasts an hour too: refresh first if under 40 minutes are left, then hand it over
        await atlassian.ensureFresh()
        // a keychain that could not be read hands nothing over: "none" would make the engine drop the found items
        await atlassian.handOverAtLaunch { [weak self] c in await self?.pushAtlassian(c, follow: false) }
        // Asana's token lasts an hour too: the same, refresh first and hand nothing over from an unreadable keychain
        await asana.ensureFresh()
        await asana.handOverAtLaunch { [weak self] c in await self?.pushAsana(c, follow: false) }
        // Dropbox's token lasts four hours: the same, refresh first and hand nothing over from an unreadable keychain
        await dropbox.ensureFresh()
        await dropbox.handOverAtLaunch { [weak self] c in await self?.pushDropbox(c, follow: false) }
        await pushMeetings()
        if let s = try? await client.indexState() { apply(s) }
    }

    /// A meeting was saved, changed or deleted: the list goes to the engine two seconds after the last change (a
    /// meeting's end brings its notes in several saves).
    func meetingsChanged() {
        meetingsSoon?.cancel()
        meetingsSoon = Task {
            try? await Task.sleep(for: .seconds(2))
            guard !Task.isCancelled else { return }
            await pushMeetings()
        }
    }

    /// Hands the engine every finished meeting it searches; it swaps the meetings into the search data when they
    /// differ from what it has. One push at a time: a push asked for meanwhile runs after it, so an older list never
    /// lands after a newer one.
    func pushMeetings() async {
        guard !meetingsPushing else {
            meetingsAgain = true
            return
        }
        meetingsPushing = true
        defer { meetingsPushing = false }
        repeat {
            meetingsAgain = false
            await pushMeetingsOnce()
        } while meetingsAgain
    }

    private var meetingsPushing = false
    private var meetingsAgain = false

    /// Only to a ready engine: one starting or stopping may belong to the account being left. A failure waits for the
    /// next change or start.
    private func pushMeetingsOnce() async {
        // `--push-meetings` keeps the meetings going with `--no-connections` (captures over a fixture folder)
        let args = LaunchArgs.all
        guard engine.state == .ready, let client = engine.client,
              !args.contains("--no-connections") || args.contains("--push-meetings") else { return }
        let list = meetings(), run = engineRun
        guard list != sentMeetings else { return }
        do {
            _ = try await client.setMeetings(list)
            if engineRun == run { sentMeetings = list }
            Log.info("past meetings handed over: \(list.count)")
        } catch {
            Log.info("past meetings not accepted: \((error as? EngineError)?.kind ?? "other")")
        }
    }

    /// Hands the engine the Linear connection (token, workspace, muted teams). A token-only change starts nothing there.
    func pushLinear(_ c: LinearConnection?, follow following: Bool = true) async {
        guard let client = engine.client else { return }
        do {
            _ = try await client.setLinear(c)
        } catch {
            Log.info("linear connection not accepted: \((error as? EngineError)?.kind ?? "other")")
            return
        }
        guard following, let s = try? await client.indexState() else { return }
        apply(s)
        if step != nil, !polling { Task { await follow(s.refresh) } }
    }

    /// Hands the engine the Google Drive connection (token, account, muted shared drives). A token-only change starts
    /// nothing there.
    func pushGDrive(_ c: GDriveConnection?, follow following: Bool = true) async {
        guard let client = engine.client else { return }
        do {
            _ = try await client.setGDrive(c)
        } catch {
            Log.info("gdrive connection not accepted: \((error as? EngineError)?.kind ?? "other")")
            return
        }
        guard following, let s = try? await client.indexState() else { return }
        apply(s)
        if step != nil, !polling { Task { await follow(s.refresh) } }
    }

    /// Hands the engine the Jira · Confluence connection (token, site, muted projects and spaces). A token-only change
    /// starts nothing there.
    func pushAtlassian(_ c: AtlassianConnection?, follow following: Bool = true) async {
        guard let client = engine.client else { return }
        do {
            _ = try await client.setAtlassian(c)
        } catch {
            Log.info("atlassian connection not accepted: \((error as? EngineError)?.kind ?? "other")")
            return
        }
        guard following, let s = try? await client.indexState() else { return }
        apply(s)
        if step != nil, !polling { Task { await follow(s.refresh) } }
    }

    /// Hands the engine the Asana connection (token, user, muted workspaces and projects). A token-only change starts
    /// nothing there.
    func pushAsana(_ c: AsanaConnection?, follow following: Bool = true) async {
        guard let client = engine.client else { return }
        do {
            _ = try await client.setAsana(c)
        } catch {
            Log.info("asana connection not accepted: \((error as? EngineError)?.kind ?? "other")")
            return
        }
        guard following, let s = try? await client.indexState() else { return }
        apply(s)
        if step != nil, !polling { Task { await follow(s.refresh) } }
    }

    /// Hands the engine the Dropbox connection (token, account, muted folders). A token-only change starts nothing
    /// there.
    func pushDropbox(_ c: DropboxConnection?, follow following: Bool = true) async {
        guard let client = engine.client else { return }
        do {
            _ = try await client.setDropbox(c)
        } catch {
            Log.info("dropbox connection not accepted: \((error as? EngineError)?.kind ?? "other")")
            return
        }
        guard following, let s = try? await client.indexState() else { return }
        apply(s)
        if step != nil, !polling { Task { await follow(s.refresh) } }
    }

    /// `--debug-control`'s `dropbox-preview`: a made-up Dropbox block and folders, for captures.
    var dropboxPreview: IndexState.DropboxInfo?
    var dropboxFoldersPreview: [DropboxFolder]?
    /// What the engine says about Dropbox (or the capture preview).
    var dropboxInfo: IndexState.DropboxInfo? { dropboxPreview ?? info?.dropbox }

    /// `--debug-control`'s `notes-preview`: a made-up notes block, for captures.
    var notesPreview: IndexState.NotesInfo?
    /// What the engine says about the note folders (or the capture preview).
    var notesInfo: IndexState.NotesInfo? { notesPreview ?? info?.notes }

    /// Hands the engine every note folder the user chose (docs/34); it saves them and reads them in the background.
    /// Returns false when the engine did not take them.
    @discardableResult
    func setNoteFolders(_ list: [NoteFolder]) async -> Bool {
        guard let client = engine.client else { return false }
        do {
            _ = try await client.setNoteFolders(list)
            Log.info("note folders handed over: \(list.count)")
        } catch {
            Log.info("note folders not accepted: \((error as? EngineError)?.kind ?? "other")")
            return false
        }
        if let s = try? await client.indexState() { apply(s) }
        // the notes go in later (the engine reads them, iCloud ones download first): look again every few seconds
        // until every chosen folder is read, so the card and the onboarding's 계속 follow without a sync
        notesWatch?.cancel()
        notesWatch = Task { [weak self] in
            for _ in 0..<200 {
                try? await Task.sleep(for: .seconds(3))
                guard !Task.isCancelled, let self, let client = self.engine.client,
                      let s = try? await client.indexState() else { return }
                self.apply(s)
                if !NotesStatus(info: self.notesInfo).reading { return }
            }
        }
        return true
    }
    @ObservationIgnored private var notesWatch: Task<Void, Never>?

    /// The top folders for the folder picker.
    func dropboxFolders() async throws -> [DropboxFolder] {
        if let dropboxFoldersPreview { return dropboxFoldersPreview }
        guard let client = engine.client else { return [] }
        return try await client.dropboxFolders()
    }

    /// `--debug-control`'s `asana-preview`: a made-up Asana block and scopes, for captures.
    var asanaPreview: IndexState.AsanaInfo?
    var asanaScopesPreview: AsanaScopes?
    /// What the engine says about Asana (or the capture preview).
    var asanaInfo: IndexState.AsanaInfo? { asanaPreview ?? info?.asana }

    /// The Asana workspaces and projects for the workspace · project picker.
    func asanaScopes() async throws -> AsanaScopes {
        if let asanaScopesPreview { return asanaScopesPreview }
        guard let client = engine.client else { return AsanaScopes(workspaces: [], projects: []) }
        return try await client.asanaScopes()
    }

    /// `--debug-control`'s `atlassian-preview`: a made-up Jira · Confluence block and scopes, for captures.
    var atlassianPreview: IndexState.AtlassianInfo?
    var atlassianScopesPreview: AtlassianScopes?
    /// What the engine says about Jira · Confluence (or the capture preview).
    var atlassianInfo: IndexState.AtlassianInfo? { atlassianPreview ?? info?.atlassian }

    /// The Jira projects and Confluence spaces for the project · space picker.
    func atlassianScopes() async throws -> AtlassianScopes {
        if let atlassianScopesPreview { return atlassianScopesPreview }
        guard let client = engine.client else { return AtlassianScopes(projects: [], spaces: []) }
        return try await client.atlassianScopes()
    }

    /// `--debug-control`'s `gdrive-preview`: a made-up Drive block and shared drives, for captures.
    var gdrivePreview: IndexState.GDriveInfo?
    var gdriveDrivesPreview: [GDriveDrive]?
    /// What the engine says about Google Drive (or the capture preview).
    var gdriveInfo: IndexState.GDriveInfo? { gdrivePreview ?? info?.gdrive }

    /// The shared drives for the drive picker.
    func gdriveDrives() async throws -> [GDriveDrive] {
        if let gdriveDrivesPreview { return gdriveDrivesPreview }
        guard let client = engine.client else { return [] }
        return try await client.gdriveDrives()
    }

    /// `--debug-control`'s `linear-preview`: a made-up Linear block and teams, for captures before a real workspace.
    var linearPreview: IndexState.LinearInfo?
    var linearTeamsPreview: [LinearTeam]?
    /// What the engine says about Linear (or the capture preview).
    var linearInfo: IndexState.LinearInfo? { linearPreview ?? info?.linear }

    /// The visible Linear teams for the team picker.
    func linearTeams() async throws -> [LinearTeam] {
        if let linearTeamsPreview { return linearTeamsPreview }
        guard let client = engine.client else { return [] }
        return try await client.linearTeams()
    }

    /// Hands the engine the Notion connection. A changed one starts a sync there: follow it like ours, without waiting
    /// for it to end (the GitHub code page waits on `pushCredentials`, which comes through here).
    func pushNotion(_ c: NotionConnection?, follow following: Bool = true) async {
        guard let client = engine.client else { return }
        do {
            _ = try await client.setNotion(token: c?.token, workspace: c?.workspace)
        } catch {
            Log.info("notion connection not accepted: \((error as? EngineError)?.kind ?? "other")")
            return
        }
        guard following, let s = try? await client.indexState() else { return }
        apply(s)
        if step != nil, !polling { Task { await follow(s.refresh) } }
    }

    func check(auto: Bool) async {
        guard let client = engine.client, let s = try? await client.indexState() else { return }
        apply(s)
        guard auto, SyncRules.shouldCheck(enabled: Self.autoSyncOn, connected: anySource, cleared: cleared,
                                          busy: step != nil || busy()) else { return }
        let c = try? await client.checkIndex()  // nil when it failed or a sync is running: the 20-hour rule decides
        if SyncRules.afterCheck(c, enabled: Self.autoSyncOn, connected: anySource, cleared: cleared, builtAt: builtAt,
                                busy: step != nil || busy(), now: Date()) {
            await refresh()
        }
    }

    func refresh() async {
        guard step == nil, let client = engine.client else { return }
        starting()
        do {
            await follow(try await client.refreshIndex())
        } catch {
            notStarted(error)
        }
    }

    /// Starts a sync of this repo and branch (nil: the saved ones); `fresh` downloads every file again instead of reusing the chunk cache.
    func rebuild(repo: String?, branch: String?, fresh: Bool) async {
        guard step == nil, let client = engine.client else { return }
        // another repo: the connection card names it until the sync ends, not the one being left
        let other = repo.map { r in info?.repo.map { $0.caseInsensitiveCompare(r) != .orderedSame } ?? true } ?? false
        switchingTo = other ? repo : nil
        defer { switchingTo = nil }
        // the sync shows the moment it is asked for: the engine's answer can take a few seconds (it looks the repo up
        // on GitHub first), and until then the card still offered 저장소 고르기
        starting()
        do {
            await follow(try await client.rebuildIndex(repo: repo, branch: branch, fresh: fresh))
        } catch {
            if (error as? EngineError)?.kind == "model" { waitingForModel = (repo, branch, fresh) }
            notStarted(error)
        }
    }

    /// A sync the engine refused for want of the search model, done once the model is here (`resumeAfterModel`).
    private var waitingForModel: (repo: String?, branch: String?, fresh: Bool)?

    /// The search model arrived and the engine runs with it: the sync it refused goes now.
    func resumeAfterModel() async {
        guard let w = waitingForModel else { return }
        waitingForModel = nil
        Log.info("index sync resumes after the model download")
        await rebuild(repo: w.repo, branch: w.branch, fresh: w.fresh)
    }

    /// Shown until the engine's answer replaces it (SettingsText.step: 준비 중).
    private func starting() {
        step = SettingsText.step(nil)
        progress = nil
        problem = nil
    }

    /// The engine did not take the sync (no answer, refused, busy): say so instead of leaving the old line as it was.
    private func notStarted(_ error: Error) {
        step = nil
        problemKind = (error as? EngineError)?.kind
        // the engine starts without the search model while it downloads; it syncs once the model is here
        problem = problemKind == "model" ? String(localized: "검색 모델을 받고 있어요. 다 받으면 동기화해요.") : SettingsText.syncNotStarted
        Log.info("index refresh not started: \(problemKind ?? "other")")
    }

    /// Deletes the search data. False when a sync is running or the engine did not answer.
    func clear() async -> Bool {
        guard step == nil, let client = engine.client, let done = try? await client.clearIndex(), done else { return false }
        if let s = try? await client.indexState() { apply(s) }
        return true
    }

    /// Set while `follow` polls, so a Notion push does not start a second poll of the same sync.
    private var polling = false

    private func follow(_ started: IndexState.Refresh) async {
        polling = true
        defer { polling = false }
        Log.info("index refresh started")
        let began = Date()
        apply(started)
        let first = firstSync != nil
        while step != nil {
            try? await Task.sleep(for: .seconds(3))
            guard let client = engine.client, let s = try? await client.indexState() else {
                step = nil  // the engine went away; it keeps the old index
                progress = nil
                return
            }
            apply(s)
        }
        if let problem { Log.info("index refresh failed: \(problem) (\(problemKind ?? "other")\(problemKind == "key" ? ", \(problemReason)" : ""))") } else { Log.info("index refreshed: \(commit?.prefix(8) ?? "-")") }
        // usage numbers (docs/16): how a sync ended, never what it synced
        if problem != nil {
            Analytics.capture(.errorShown(kind: AErrorKind(kind: problemKind), place: .sync))
        } else if info?.refresh.state == "done" {  // not a cancelled or cleared one
            Analytics.capture(.indexReady(first: first, chunks: .of(info?.chunks ?? 0), seconds: Int(Date().timeIntervalSince(began))))
        }
    }

    private func apply(_ s: IndexState) {
        info = s
        builtAt = Self.date(s.builtAt)
        commit = s.commit
        if let c = s.connected { connected = c }
        cleared = s.cleared ?? false
        apply(s.refresh)
        // Notion refused the token on the last sync: ask for a new one (NotionLink keeps it to one try in ten minutes)
        if s.notion?.refused == true, notion.connected, step == nil {
            Task { await notion.renew() }
        }
        // Linear refused the token: one forced refresh (LinearLink keeps it to one try in ten minutes)
        if s.linear?.refused == true, linear.connected, step == nil {
            Task { await linear.renew() }
        }
        // Drive refused the token: renew at once, also mid-sync (the engine waits for the new token, docs/19 10-1절)
        if s.gdrive?.refused == true, gdrive.connected {
            Task { await gdrive.renew() }
        }
        // Atlassian refused the token: renew at once, also mid-sync (the engine waits for the new token, docs/31 10-2절;
        // AtlassianLink keeps it to once a minute)
        if s.atlassian?.refused == true, atlassian.connected {
            Task { await atlassian.renew() }
        }
        // Asana refused the token: renew at once, also mid-sync (the engine waits for the new token; AsanaLink keeps it
        // to once a minute)
        if s.asana?.refused == true, asana.connected {
            Task { await asana.renew() }
        }
        // Dropbox refused the token: renew at once, also mid-sync (the engine waits for the new token)
        if s.dropbox?.refused == true, dropbox.connected {
            Task { await dropbox.renew() }
        }
    }

    private func apply(_ r: IndexState.Refresh) {
        let busy = r.state == "running" || r.state == "clearing"
        step = busy ? SettingsText.step(r.step) : nil
        rawStep = busy ? r.step : nil
        progress = busy ? r.progress.map { ($0.done, $0.total) } : nil
        if r.state == "failed" {
            problemKind = r.errorKind
            problemReason = SyncRules.githubReason(r.error)
            problem = r.errorKind == "network" ? String(localized: "인터넷 연결이 끊겨 동기화하지 못했어요") : String(localized: "동기화하지 못했어요")
        } else if r.state == "idle" || r.state == "done" {
            problem = nil
            problemKind = nil
            if r.state == "done" { partial = SettingsText.syncWarning(r.warnings ?? []) }
        }
    }

    /// meta.json's built_at: "2026-10-23T01:02:56+0900", or a bare date from older indexes.
    private static func date(_ s: String?) -> Date? {
        guard let s else { return nil }
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = s.count > 10 ? "yyyy-MM-dd'T'HH:mm:ssZ" : "yyyy-MM-dd"
        return f.date(from: s)
    }
}
