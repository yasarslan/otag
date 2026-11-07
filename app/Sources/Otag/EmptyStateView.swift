import OtagKit
import SwiftUI

/// What fills the main area with nothing selected.
///
/// A card with one button and three lines, and each line names something
/// that is actually on screen: the list on the left, the button in a
/// session's header, and the dragging. Not a shortcut sheet — somebody
/// opening otag for the second time needs to know where things are, not
/// which keys exist.
struct EmptyStateView: View {
    let state: ConnectionState
    let newSession: () -> Void
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        VStack(spacing: 0) {
            switch state {
            case .connecting:
                Card {
                    ProgressView()
                    Text(Strings.t("daemon.connectingTo"))
                        .foregroundStyle(.secondary)
                }

            case .connected:
                Card {
                    Image(systemName: "terminal")
                        .font(.system(size: 24, weight: .medium))
                        .foregroundStyle(Color.accentColor)
                        .frame(width: 52, height: 52)
                        .background(Color.accentColor.opacity(0.12), in: RoundedRectangle(cornerRadius: 15))
                        .accessibilityHidden(true)

                    Text(Strings.t("empty.title"))
                        .font(.title2.weight(.semibold))
                        .multilineTextAlignment(.center)

                    Button {
                        newSession()
                    } label: {
                        Label(Strings.t("empty.new"), systemImage: "plus")
                    }
                    .primaryActionStyle(prominent: true)
                    .controlSize(.large)

                    VStack(alignment: .leading, spacing: 10) {
                        line("sidebar.left", Strings.t("empty.sidebar"))
                        line("arrow.left.arrow.right", Strings.t("empty.switch"))
                        line("rectangle.split.2x2", Strings.t("empty.arrange"))
                    }
                    .frame(maxWidth: 340, alignment: .leading)
                }

            case let .failed(message):
                Card {
                    Image(systemName: "exclamationmark.triangle")
                        .font(.system(size: 30, weight: .light))
                        .foregroundStyle(.tertiary)
                    Text(Strings.t("daemon.notConnected"))
                        .font(.title3)
                    Text(message)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: 340)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background {
            RadialGradient(
                colors: [Color.accentColor.opacity(colorScheme == .dark ? 0.17 : 0.12), .clear],
                center: .topLeading,
                startRadius: 0,
                endRadius: 650
            )
            .allowsHitTesting(false)
        }
    }

    private func line(_ symbol: String, _ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Image(systemName: symbol)
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(width: 16)
            Text(.init(text))
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    /// The one card the empty state ever is.
    private struct Card<Content: View>: View {
        @ViewBuilder let content: Content

        var body: some View {
            VStack(spacing: 16) {
                content
            }
            .padding(28)
            .frame(width: 420)
            .cardBackground(radius: 20)
        }
    }
}
