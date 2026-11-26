import SwiftUI

/// Settings → Connections' scene (2026-11-16, the landing's connection beam at settings size): the on-screen Tacie on
/// the left, a line to each connected source, and the sources not connected yet waiting unjoined in a fainter column.
/// The row under the pointer darkens its line; while a sync runs, a spark of the face's light runs along the lines.
/// Built from plain values so the render tests can draw it without an engine (SettingsSceneRenderTests).
struct ConnectionsScene: View {
    enum Source: String, CaseIterable, Identifiable {
        case github, notion, linear, gdrive, atlassian, asana, dropbox, notes
        var id: String { rawValue }

        /// The open edition has neither Tacie's server nor its registered connection apps: the services that take a
        /// pasted personal token (`PastedTokens`), and the note folder. Google Drive and Dropbox would need the
        /// person's own registered app, so they stay out (docs/41 0절, 2026-11-23).
        static func available(open: Bool, dev: Bool) -> [Source] {
            guard open else { return allCases }
            return [.github, .notion, .linear, .atlassian, .asana, .notes]
        }
        static var available: [Source] { available(open: Build.isOpen, dev: Build.isDev) }
        /// 처음 써 보기's "+" tools after GitHub (which it lists on its own), in its order.
        static func firstStepsTools(open: Bool, dev: Bool) -> [Source] {
            let shown = available(open: open, dev: dev)
            return [Source.notion, .linear, .gdrive, .atlassian, .asana, .dropbox, .notes].filter(shown.contains)
        }

        var name: String {
            switch self {
            case .github: "GitHub"
            case .notion: String(localized: "노션")
            case .linear: "Linear"
            case .gdrive: "Google Drive"
            case .atlassian: "Jira · Confluence"
            case .asana: "Asana"
            case .dropbox: "Dropbox"
            case .notes: "Obsidian"
            }
        }

        var logos: [String] {
            switch self {
            case .github: ["ServiceGitHub"]
            case .notion: ["ServiceNotion"]
            case .linear: ["ServiceLinear"]
            case .gdrive: ["ServiceGoogleDrive"]
            case .atlassian: ["ServiceJira", "ServiceConfluence"]
            case .asana: ["ServiceAsana"]
            case .dropbox: ["ServiceDropbox"]
            case .notes: ["ServiceObsidian"]
            }
        }

        /// The line under the scene for a source not connected yet, whole so each language can say it its own way.
        var offCaption: String {
            switch self {
            case .github: String(localized: "GitHub를 연결하면 코드와 이슈·PR에서도 찾아요.")
            case .notion: String(localized: "노션을 연결하면 팀 문서와 위키에서도 찾아요.")
            case .linear: String(localized: "Linear를 연결하면 이슈와 프로젝트에서도 찾아요.")
            case .gdrive: String(localized: "Google Drive를 연결하면 문서, 시트, 슬라이드에서도 찾아요.")
            case .atlassian: String(localized: "Jira · Confluence를 연결하면 이슈와 팀 문서에서도 찾아요.")
            case .asana: String(localized: "Asana를 연결하면 할 일과 프로젝트에서도 찾아요.")
            case .dropbox: String(localized: "Dropbox를 연결하면 공유 폴더의 파일에서도 찾아요.")
            case .notes: String(localized: "Obsidian vault를 고르면 이 Mac의 노트에서도 찾아요.")
            }
        }
    }

    /// Which sources are connected, in the page's order.
    let on: [Source: Bool]
    var focus: Source? = nil
    /// The sync line while a sync runs ("동기화하는 중 · 노션 페이지 받는 중"); nil otherwise.
    var syncing: String? = nil

    @Environment(\.reducesMotion) private var reduceMotion

    private var joined: [Source] { Source.available.filter { on[$0] == true } }
    private var waiting: [Source] { Source.available.filter { on[$0] != true } }

    private var caption: String {
        if let syncing { return syncing }
        if let focus {
            return on[focus] == true
                ? String(localized: "회의 중 질문이 나오면 \(focus.name)에서 내 계정으로 볼 수 있는 내용만 찾아요.")
                : focus.offCaption
        }
        if joined.isEmpty { return String(localized: "서비스를 연결하면 회의 중에 거기서 답을 찾아요.") }
        return String(localized: "\(joined.map(\.name).joined(separator: ", "))에서 찾고 있어요.")
    }

    var body: some View {
        SettingsSceneFrame(caption: caption) {
            GeometryReader { g in
                let lay = Layout(size: g.size, joined: joined, waiting: waiting)
                ZStack(alignment: .topLeading) {
                    beams(lay)
                    ForEach(Source.available) { s in
                        node(s).position(lay.point(s))
                    }
                    ScenePill(mood: syncing != nil ? .thinking : .listening) {
                        Text(syncing != nil ? String(localized: "받아 오는 중") : "Tacie")
                            .uiFont(12.5, weight: .medium).foregroundStyle(Theme.ink)
                    }
                    .fixedSize()
                    .position(lay.hub)
                }
            }
        }
        .animation(Theme.anim(Theme.enter, reduce: reduceMotion), value: focus)
        .animation(Theme.anim(Theme.enter, reduce: reduceMotion), value: on)
    }

    // MARK: - drawing

    private struct Layout {
        let hub: CGPoint
        let spots: [Source: CGPoint]

        init(size: CGSize, joined: [Source], waiting: [Source]) {
            let top: CGFloat = 26, bottom = size.height - 52  // node centres stay clear of the frame and the caption
            let mid = (top + bottom) / 2
            hub = CGPoint(x: size.width * 0.2, y: mid)
            var spots: [Source: CGPoint] = [:]
            // more than three in a group zigzag over two columns, so six or eight still fit the scene's height
            func group(_ list: [Source], left: CGFloat, right: CGFloat) {
                let zig = list.count > 3
                let gap = list.count > 1 ? min(zig ? 30 : 40, (bottom - top) / CGFloat(list.count - 1)) : 0
                let first = mid - gap * CGFloat(list.count - 1) / 2
                for (i, s) in list.enumerated() {
                    let x = zig ? (i.isMultiple(of: 2) ? left : right) : (left + right) / 2
                    spots[s] = CGPoint(x: size.width * x, y: first + gap * CGFloat(i))
                }
            }
            if joined.isEmpty {
                group(waiting, left: 0.56, right: 0.7)
            } else {
                group(joined, left: 0.52, right: 0.64)
                group(waiting, left: 0.82, right: 0.92)
            }
            self.spots = spots
        }

        func point(_ s: Source) -> CGPoint { spots[s] ?? hub }

        /// The curve from the face to a node: out flat from the face, in flat to the node.
        func curve(to s: Source) -> (CGPoint, CGPoint, CGPoint, CGPoint) {
            let end = point(s), a = CGPoint(x: hub.x + 40, y: hub.y), d = CGPoint(x: end.x - 20, y: end.y)
            return (a, CGPoint(x: a.x + (d.x - a.x) * 0.45, y: a.y), CGPoint(x: d.x - (d.x - a.x) * 0.45, y: d.y), d)
        }
    }

    private func path(_ c: (CGPoint, CGPoint, CGPoint, CGPoint)) -> Path {
        Path { p in p.move(to: c.0); p.addCurve(to: c.3, control1: c.1, control2: c.2) }
    }

    private static func at(_ c: (CGPoint, CGPoint, CGPoint, CGPoint), _ t: CGFloat) -> CGPoint {
        let u = 1 - t
        let x = u * u * u * c.0.x + 3 * u * u * t * c.1.x + 3 * u * t * t * c.2.x + t * t * t * c.3.x
        let y = u * u * u * c.0.y + 3 * u * u * t * c.1.y + 3 * u * t * t * c.2.y + t * t * t * c.3.y
        return CGPoint(x: x, y: y)
    }

    @ViewBuilder private func beams(_ lay: Layout) -> some View {
        let joined = joined
        ZStack {
            ForEach(joined) { s in
                path(lay.curve(to: s))
                    .stroke(focus == s ? Theme.ink : Theme.ink4, lineWidth: focus == s ? 1.6 : 1.2)
            }
            if syncing != nil, !reduceMotion, !joined.isEmpty {
                TimelineView(.animation(minimumInterval: 1 / 30)) { ctx in
                    let t = ctx.date.timeIntervalSinceReferenceDate
                    Canvas { gc, _ in
                        for (i, s) in joined.enumerated() {
                            // from the source to the face, each line a little behind the one above
                            let k = CGFloat((t / 1.6 + Double(i) * 0.27).truncatingRemainder(dividingBy: 1))
                            let p = Self.at(lay.curve(to: s), 1 - k)
                            let r: CGFloat = 3
                            gc.addFilter(.shadow(color: Theme.eyeGlow.opacity(0.8), radius: 3))
                            gc.fill(Path(ellipseIn: CGRect(x: p.x - r, y: p.y - r, width: 2 * r, height: 2 * r)), with: .color(Theme.eyeGlow))
                        }
                    }
                }
            }
        }
    }

    private func node(_ s: Source) -> some View {
        let joined = on[s] == true
        return HStack(spacing: 1) {
            ForEach(s.logos, id: \.self) { logo in
                Image(logo).resizable().scaledToFit().frame(width: s.logos.count > 1 ? 11 : 16, height: s.logos.count > 1 ? 11 : 16)
            }
        }
        .opacity(joined ? 1 : 0.55)
        .frame(width: 32, height: 32)
        .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(joined ? Theme.logoTile : .clear))
        .overlay {
            if joined {
                RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(Theme.hairline)
            } else {
                // under the pointer the waiting source's frame darkens; it has no line until it is connected
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .strokeBorder(focus == s ? AnyShapeStyle(Theme.ink3) : AnyShapeStyle(Theme.ink4), style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
            }
        }
        .shadow(color: .black.opacity(joined ? 0.08 : 0), radius: 6, y: 2)
        .opacity(joined || focus == s ? 1 : 0.7)
        .scaleEffect(focus == s ? 1.12 : 1)
    }
}
