import SwiftUI

/// A model download in the sidebar (voice model, on-device speech recognition): a bar while it runs, a red dot and Try
/// again when it failed.
struct ModelRow: View {
    let row: VoiceRow
    let retry: () -> Void

    var body: some View {
        if row.failed {
            HStack(alignment: .center, spacing: 8) {
                Circle().fill(Theme.warn).frame(width: 6, height: 6)
                VStack(alignment: .leading, spacing: 2) {
                    Text(row.title).uiFont(12, weight: .semibold).foregroundStyle(Theme.ink)
                    Text(row.detail).uiFont(11).foregroundStyle(Theme.ink3).lineLimit(2)
                }
                Spacer(minLength: 4)
                Button(action: retry) { OutlineButton(label: String(localized: "modelRow.retry", defaultValue: "다시 받기")) }.buttonStyle(PressStyle())
            }
            .padding(.horizontal, 14).padding(.vertical, 8)
        } else {
            VStack(alignment: .leading, spacing: 5) {
                HStack(spacing: 6) {
                    Text(row.title).uiFont(12, weight: .semibold).foregroundStyle(Theme.ink)
                    Spacer(minLength: 4)
                    if let percent = row.percent {
                        Text(percent).uiFont(11.5, weight: .medium, digits: true).foregroundStyle(Theme.ink3)
                    }
                }
                SyncBar(fraction: row.fraction ?? 0)
                Text(row.detail).uiFont(11).foregroundStyle(Theme.ink3).lineLimit(2)
            }
            .padding(.horizontal, 14).padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

/// The on-device speech recognition's row: progress or failure while it downloads, then the done mark for a moment.
/// A click opens 설정 → 개인정보, where the switch is.
struct STTSidebarRow: View {
    let stt: LocalSTT
    let open: () -> Void
    @State private var finished = false

    var body: some View {
        Group {
            if let row = LocalSTTCopy.sidebar(stt.phase) {
                ModelRow(row: row, retry: { stt.retry() })
            } else if finished {
                DoneMark(text: LocalSTTCopy.doneText).padding(.horizontal, 14).padding(.vertical, 4)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .contentShape(Rectangle())
        .onTapGesture(perform: open)
        .onChange(of: stt.phase) { old, new in
            guard new == .ready, LocalSTTCopy.sidebar(old) != nil else { return }
            finished = true
            Task {
                try? await Task.sleep(for: .seconds(3))
                finished = false
            }
        }
    }
}

/// The main window's sync bar: a thin track that fills as the sync goes, sliding rather than jumping.
struct SyncBar: View {
    let fraction: Double
    @Environment(\.reducesMotion) private var reduceMotion

    var body: some View {
        GeometryReader { g in
            ZStack(alignment: .leading) {
                Capsule().fill(Theme.ink.opacity(0.08))
                Capsule().fill(Theme.ink.opacity(0.75))
                    .frame(width: max(4, g.size.width * min(1, max(0, fraction))))
            }
        }
        .frame(height: 4)
        .animation(Theme.anim(.easeOut(duration: 0.6), reduce: reduceMotion), value: fraction)
    }
}
