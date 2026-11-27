import SwiftUI

/// What the user is being asked after flipping the on-device speech recognition switch.
enum STTRequest: Equatable {
    case consent
    case space(Int64)
}

/// The two alerts of docs/22 4-2: the confirmation before a 0.6GB download, and "not enough room".
private struct STTAlerts: ViewModifier {
    let stt: LocalSTT
    @Binding var request: STTRequest?

    func body(content: Content) -> some View {
        content
            .alert(LocalSTTCopy.consentTitle, isPresented: Binding(get: { request == .consent }, set: { if !$0, request == .consent { request = nil } })) {
                Button(LocalSTTCopy.consentButton) {
                    request = nil
                    if case .needsSpace(let more) = stt.turnOn(consented: true) { request = .space(more) }
                }
                Button(LocalSTTCopy.cancel, role: .cancel) { request = nil }
            } message: {
                Text(LocalSTTCopy.consentBody)
            }
            .alert(LocalSTTCopy.spaceTitle, isPresented: Binding(get: { if case .space = request { true } else { false } },
                                                                 set: { if !$0 { request = nil } })) {
                Button(LocalSTTCopy.spaceButton) { request = nil }
            } message: {
                if case .space(let more) = request { Text(LocalSTTCopy.spaceBody(more: more)) }
            }
    }
}

extension View {
    func sttAlerts(_ stt: LocalSTT, request: Binding<STTRequest?>) -> some View {
        modifier(STTAlerts(stt: stt, request: request))
    }
}

/// 설정 → 회의 → 회의 중: where speech recognition runs, 서버 or 온디바이스 (2026-11-18; a switch before), its download and
/// 음성 인식 모델 삭제. Picking 온디바이스 asks before the 0.6GB download, as the switch did.
struct LocalSTTRow: View {
    let stt: LocalSTT
    @State private var request: STTRequest?
    @State private var confirmDelete = false

    private var on: Binding<Bool> {
        Binding(get: { LocalSTTCopy.isOn(stt.phase) },
                set: { wanted in
                    guard wanted != LocalSTTCopy.isOn(stt.phase) else { return }
                    if wanted {
                        switch stt.turnOn(consented: false) {
                        case .askConsent: request = .consent
                        case .needsSpace(let more): request = .space(more)
                        case .started, .ready, .unsupported: break
                        }
                    } else {
                        stt.turnOff()
                    }
                })
    }

    var body: some View {
        let phase = stt.phase
        let fraction: Double? = if case .downloading(let done, let total) = phase, total > 0 { min(1, Double(done) / Double(total)) } else { nil }
        SettingsRow(title: LocalSTTCopy.rowTitle, desc: LocalSTTCopy.detail(phase, serverFallback: LocalSTTCopy.serverFallback), bar: fraction) {
            ChoiceStrip(choices: [(false, LocalSTTCopy.serverChoice), (true, LocalSTTCopy.deviceChoice)], value: on)
                .disabled(phase == .unsupported)
                .opacity(phase == .unsupported ? 0.4 : 1)
        }
        .sttAlerts(stt, request: $request)
        if LocalSTTCopy.isOn(phase) {
            Button { confirmDelete = true } label: {
                SettingsRow(title: LocalSTTCopy.deleteRow, desc: LocalSTTCopy.deleteDesc, titleColor: Theme.warn) { Chevron() }
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .alert(LocalSTTCopy.deleteTitle, isPresented: $confirmDelete) {
                Button(LocalSTTCopy.deleteButton, role: .destructive) { stt.deleteModel() }
                Button(LocalSTTCopy.cancel, role: .cancel) {}
            } message: {
                Text(LocalSTTCopy.deleteBody)
            }
        }
    }
}

/// 2단계's choice of speech recognition (2026-11-18, docs/22 9절): two cards, 서버 음성 인식 picked by default, each with
/// what it costs. Hidden under 16 GB. Picking 온디바이스 needs no confirmation (the size is on the card); the download
/// waits for the search model.
struct OnboardingSTTRow: View {
    let stt: LocalSTT
    @State private var request: STTRequest?

    private func pick(device: Bool) {
        guard device != LocalSTTCopy.isOn(stt.phase) else { return }
        if !device { stt.turnOff(); return }
        if case .needsSpace(let more) = stt.turnOn(consented: true, afterSearchModel: true) { request = .space(more) }
    }

    var body: some View {
        let device = LocalSTTCopy.isOn(stt.phase)
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(LocalSTTCopy.onboardingSection).font(.system(size: 13, weight: .bold)).foregroundStyle(Theme.ink)
                Text(LocalSTTCopy.onboardingSectionSub).font(.system(size: 12)).foregroundStyle(Theme.ink3)
            }
            HStack(alignment: .top, spacing: 10) {
                STTChoiceCard(name: LocalSTTCopy.serverName, tag: LocalSTTCopy.defaultTag, line: LocalSTTCopy.serverLine,
                              pros: LocalSTTCopy.serverPros, cons: LocalSTTCopy.serverCons, on: !device) { pick(device: false) }
                STTChoiceCard(name: LocalSTTCopy.title, tag: nil, line: LocalSTTCopy.deviceLine,
                              pros: [], cons: LocalSTTCopy.deviceCons, on: device) { pick(device: true) }
            }
            .fixedSize(horizontal: false, vertical: true)
            Text(LocalSTTCopy.onboardingFoot).font(.system(size: 11.5)).foregroundStyle(Theme.ink3)
                .fixedSize(horizontal: false, vertical: true).padding(.leading, 4)
        }
        .padding(.top, 16)
        .sttAlerts(stt, request: $request)
    }
}

/// One of the two cards: a radio dot, the name, one line of what it is for, then what it does well (filled dots) and
/// what it costs (hollow dots). The whole card is one press.
private struct STTChoiceCard: View {
    let name: String
    let tag: String?
    let line: String
    let pros: [String]
    let cons: [String]
    let on: Bool
    let action: () -> Void
    @Environment(\.reducesMotion) private var reduceMotion

    var body: some View {
        Button { withAnimation(Theme.anim(Theme.snappy, reduce: reduceMotion)) { action() } } label: {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 8) {
                    ZStack {
                        Circle().strokeBorder(on ? Theme.ink : Theme.ink4, lineWidth: 1.5)
                        if on { Circle().fill(Theme.ink).frame(width: 8, height: 8) }
                    }
                    .frame(width: 16, height: 16)
                    Text(name).font(.system(size: 14, weight: .semibold)).foregroundStyle(Theme.ink)
                    if let tag {
                        Text(tag).font(.system(size: 11, weight: .semibold)).foregroundStyle(Theme.ink3)
                            .padding(.horizontal, 7).frame(height: 17).background(Capsule().fill(Theme.fill))
                    }
                }
                Text(line).font(.system(size: 11.5)).foregroundStyle(Theme.ink2)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.leading, 24).padding(.top, 2).padding(.bottom, 8)
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(pros, id: \.self) { point($0, good: true) }
                    ForEach(cons, id: \.self) { point($0, good: false) }
                }
                .padding(.leading, 24)
                Spacer(minLength: 0)
            }
            .padding(12)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
                .strokeBorder(on ? Theme.ink : Theme.ink.opacity(0.1), lineWidth: on ? 1.5 : 1))
            .contentShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? [.isSelected] : [])
    }

    private func point(_ text: String, good: Bool) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 7) {
            Circle().fill(good ? Theme.ink : Color.clear)
                .overlay(Circle().strokeBorder(good ? Color.clear : Theme.ink4, lineWidth: 1))
                .frame(width: 5, height: 5).offset(y: -2)
            Text(text).font(.system(size: 11.5)).foregroundStyle(good ? AnyShapeStyle(Theme.ink2) : AnyShapeStyle(Theme.ink3))
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
