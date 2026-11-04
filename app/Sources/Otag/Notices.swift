import SwiftUI

/// What otag says about an answer it could not deliver.
///
/// An answer is typed into a running agent, so otag never claims one worked:
/// if the prompt is still up afterwards, this is what says so. It sits just
/// above the limits card, where the question it failed to answer was.
struct AnswerProblemView: View {
    let problem: String
    let dismiss: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: Chrome.gap) {
            Image(systemName: "exclamationmark.triangle")
                .foregroundStyle(.secondary)
            Text(problem)
                .font(.caption)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            DismissButton(dismiss: dismiss)
        }
        .padding(10)
        .cardBackground(radius: Chrome.controlRadius)
        .padding(.horizontal, 10)
        .padding(.bottom, 2)
    }
}

/// One line across the bottom of the window: what otag just did.
///
/// A handoff, something the night shift sent, or a session the window had to
/// stop showing to make room. A handoff's line goes by itself once the
/// session it opened is on screen, because by then the session says it
/// better; anything else stays until it is dismissed.
struct NoticeBar: View {
    let notice: String
    let dismiss: () -> Void

    var body: some View {
        HStack(spacing: Chrome.gap) {
            Image(systemName: "info.circle")
                .foregroundStyle(.secondary)
            Text(notice)
                .font(.caption)
                .lineLimit(2)
            Spacer(minLength: 0)
            DismissButton(dismiss: dismiss)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
        .chromeBackground()
        .overlay(alignment: .top) { Divider() }
    }
}

/// The small cross on a notice.
private struct DismissButton: View {
    let dismiss: () -> Void

    var body: some View {
        Button(action: dismiss) {
            Image(systemName: "xmark")
                .font(.system(size: 9, weight: .bold))
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
        .accessibilityLabel("Dismiss")
    }
}
