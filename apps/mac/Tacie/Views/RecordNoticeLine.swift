import SwiftUI

/// Recording notice (docs/35 6-1): asking, then copied.
enum RecordNotice: Equatable { case ask, copied }

/// 녹음 알림 under the pill (docs/35 6-1): the question and 알림 문구 복사, then a check and where to paste it.
struct RecordNoticeLine: View {
    let notice: RecordNotice
    let copy: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            if notice == .copied {
                Image(systemName: "checkmark").font(.system(size: 11, weight: .bold)).foregroundStyle(Theme.live)
                Text("복사했어요. 회의 채팅에 붙여 넣으세요.").font(Theme.chip).foregroundStyle(Theme.ink2).lineLimit(1).fixedSize()
            } else {
                Text("녹음 중이에요. 참석자에게 알렸나요?").font(Theme.chip).foregroundStyle(Theme.ink2).lineLimit(1).fixedSize()
                Button(action: copy) {
                    Text("알림 문구 복사").font(.system(size: 12, weight: .semibold)).foregroundStyle(Theme.ink)
                        .padding(.horizontal, 9).frame(height: 24).background(Theme.fill, in: Capsule()).fixedSize()
                }
                .buttonStyle(.plain)
                .help("참석자에게 알릴 문구를 복사해요")
            }
        }
    }
}
