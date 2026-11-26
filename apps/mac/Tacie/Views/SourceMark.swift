import SwiftUI

/// Where a source lives, on its chip: the service's logo (GitHub, Notion, Linear, Google Drive), drawn as is, or a
/// plain symbol where there is no logo (Slack, the onboarding's example files).
struct SourceMark: View {
    @Environment(\.colorScheme) private var scheme
    let card: Card
    var size: CGFloat = 12
    var body: some View {
        // a mark not in the asset catalog (yet) falls back to the symbol
        if let logo = card.logo, NSImage(named: logo) != nil {
            Image(ServiceLogo.name(logo, scheme)).resizable().scaledToFit().frame(width: size, height: size)
        } else if let symbol = card.symbol ?? (card.inRepo ? (card.isDoc ? "doc.text" : "chevron.left.forwardslash.chevron.right") : nil) {
            Image(systemName: symbol).font(.system(size: size - 2, weight: .semibold)).foregroundStyle(Theme.ink3)
        }
    }
}
