import SwiftUI

/// The main open-source parts of the app and the engine. Every license text, the Rust crates included, ships in
/// ThirdPartyNotices.txt (apps/mac/scripts/third-party-notices.py writes it).
enum OpenSource {
    static let entries: [(name: String, license: String, url: URL)] = [
        ("whisper.cpp", "MIT", URL(string: "https://github.com/ggml-org/whisper.cpp/blob/master/LICENSE")!),
        ("Whisper large-v3-turbo", "MIT", URL(string: "https://github.com/openai/whisper/blob/main/LICENSE")!),
        ("Silero VAD", "MIT", URL(string: "https://github.com/snakers4/silero-vad/blob/master/LICENSE")!),
        ("BAAI/bge-m3", "MIT", URL(string: "https://huggingface.co/BAAI/bge-m3")!),
        ("3D-Speaker (ERes2Net, CAM++)", "Apache-2.0", URL(string: "https://github.com/modelscope/3D-Speaker/blob/main/LICENSE")!),
        ("ONNX Runtime", "MIT", URL(string: "https://onnxruntime.ai")!),
        ("MLX", "MIT", URL(string: "https://github.com/ml-explore/mlx/blob/main/LICENSE")!),
        ("kaldi-native-fbank", "Apache-2.0", URL(string: "https://github.com/csukuangfj/kaldi-native-fbank/blob/master/LICENSE")!),
        ("Oniguruma", "BSD-2-Clause", URL(string: "https://github.com/kkos/oniguruma/blob/master/COPYING")!),
        ("SQLite", String(localized: "퍼블릭 도메인"), URL(string: "https://www.sqlite.org/copyright.html")!),
        ("Sparkle", "MIT", URL(string: "https://github.com/sparkle-project/Sparkle/blob/2.x/LICENSE")!),
        ("PostHog iOS", "MIT", URL(string: "https://github.com/PostHog/posthog-ios/blob/main/LICENSE")!),
    ]

    /// Every notice, as it ships in the app.
    static var notices: URL? { Bundle.main.url(forResource: "ThirdPartyNotices", withExtension: "txt") }
}

struct LicensesPage: View {
    let back: () -> Void

    var body: some View {
        DetailHeader(back: String(localized: "일반"), action: back)
        PageHeader(title: String(localized: "오픈소스 라이선스"), desc: String(localized: "항목을 누르면 그 라이선스를 열어요."))
        SettingsSection(title: String(localized: "주요 오픈소스")) {
            ForEach(OpenSource.entries, id: \.name) { entry in
                Button { NSWorkspace.shared.open(entry.url) } label: {
                    SettingsRow(title: entry.name, desc: entry.license) {
                        Image(systemName: "arrow.up.right").uiFont(11, weight: .semibold).foregroundStyle(Theme.ink4)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        if let notices = OpenSource.notices {
            FooterLinks {
                FooterLink(label: String(localized: "전체 라이선스 고지 열기")) { NSWorkspace.shared.open(notices) }
            }
            .padding(.top, 14)
        }
    }
}
