import Charts
import OtagKit
import SwiftUI

// The pieces the limits card and its popover are both built from. The card
// itself and the popover live in `LimitsCard.swift`; these are the parts
// that say one number, and are used by both.

/// Ask the daemon for the numbers again.
///
/// Always there, beside the numbers it refreshes, because the one moment a
/// person wants it is the moment the numbers look wrong.
struct UsageRefresh: View {
    let store: SessionStore

    /// How far the icon has turned, and the task turning it.
    ///
    /// One finite step at a time rather than one endless animation: an
    /// animation told to repeat forever is only stopped by changing the
    /// value it animates, and the value it had already reached was the
    /// value it was going to be set to. So it never stopped.
    @State private var turn = 0.0
    @State private var spinning: Task<Void, Never>?
    @State private var justUpdated = false

    /// How long one step of the turn takes.
    private static let step = Duration.milliseconds(250)

    var body: some View {
        HStack(spacing: 4) {
            if justUpdated {
                Text("Updated")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .transition(.opacity)
            }
            Button {
                store.refreshUsage()
            } label: {
                Image(systemName: "arrow.clockwise")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .rotationEffect(.degrees(turn))
                    // A hit area a person can actually hit: the glyph itself
                    // is ten points across, and this is the whole square.
                    .frame(width: 20, height: 20)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.borderless)
            .help("Re-read the limits")
            .accessibilityLabel("Refresh usage")
        }
        .onChange(of: store.refreshingUsage) { _, refreshing in
            spin(refreshing)
        }
        .onChange(of: store.usageUpdatedAt) {
            withAnimation { justUpdated = true }
            Task {
                try? await Task.sleep(for: .seconds(2))
                withAnimation { justUpdated = false }
            }
        }
        .onDisappear {
            spinning?.cancel()
            spinning = nil
        }
    }

    /// Turn the icon while a re-read is in flight, and stop when it is not.
    ///
    /// Each step is an animation that ends, so the icon stops as soon as
    /// nothing is asking it to turn: the store clears its flag when the
    /// report arrives, and gives up by itself if none does.
    private func spin(_ refreshing: Bool) {
        spinning?.cancel()
        guard refreshing else {
            withAnimation(.easeOut(duration: 0.2)) { turn = 0 }
            return
        }

        spinning = Task { @MainActor in
            while !Task.isCancelled, store.refreshingUsage {
                withAnimation(.linear(duration: 0.25)) { turn += 120 }
                try? await Task.sleep(for: Self.step)
            }
            withAnimation(.easeOut(duration: 0.2)) { turn = 0 }
        }
    }
}

/// One window: what the agent last reported, and when it resets.
struct UsageRowView: View {
    let row: UsageRow

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 4) {
                Text(row.agent.label)
                    .lineLimit(1)
                Text(row.window.label)
                    .foregroundStyle(.secondary)
                Spacer(minLength: 4)
                Text(used)
                    .monospacedDigit()
                    .foregroundStyle(colour)
            }
            .font(.caption)

            Meter(fraction: row.fraction, colour: colour)

            if let resets = Readable.resetClock(row.resetsAt) {
                Text("resets \(resets)")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
    }

    /// The percentage, or a dash when otag has never had a reading of this
    /// window. A dash says what no percentage can say.
    private var used: String {
        guard let percent = row.usedPercent else { return Readable.noReading }
        return "\(percent)%"
    }

    /// A reading is coloured by what it says and by nothing else.
    private var colour: Color {
        guard row.usedTenths != nil else { return .secondary }
        switch row.level {
        case .calm: return .secondary
        case .amber: return .orange
        case .red: return .red
        }
    }
}

/// A thin bar. No animation, no gradient, nothing that fills up as you
/// watch: it is a number you glance at.
struct Meter: View {
    let fraction: Double
    let colour: Color

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Capsule()
                    .fill(.quaternary)
                Capsule()
                    .fill(colour.opacity(0.7))
                    .frame(width: geometry.size.width * min(max(fraction, 0), 1))
            }
        }
        .frame(height: 3)
    }
}

/// How much of the limits was gone, over time.
struct UsageChart: View {
    let samples: [UsageSample]

    var body: some View {
        if samples.count < 2 {
            Text("A chart needs a few readings. otag keeps them across restarts, so this "
                + "fills in as the agents work.")
                .font(.caption)
                .foregroundStyle(.secondary)
        } else {
            Chart(samples) { sample in
                LineMark(
                    x: .value("When", sample.date),
                    y: .value("Used", sample.usedPercent)
                )
                .foregroundStyle(by: .value("Window", label(sample)))
                .interpolationMethod(.monotone)
            }
            .chartYScale(domain: 0 ... 100)
            .chartYAxis {
                AxisMarks(values: [0, 50, 100]) { value in
                    AxisGridLine()
                    AxisValueLabel {
                        if let percent = value.as(Int.self) {
                            Text("\(percent)%")
                        }
                    }
                }
            }
            .chartLegend(position: .bottom, spacing: 6)
            .frame(height: 140)
        }
    }

    private func label(_ sample: UsageSample) -> String {
        "\(sample.agent.label) \(sample.window.label)"
    }
}

/// How much of a session's context is in use.
///
/// Only a bar when otag knows how large the model's context is; otherwise
/// the count alone, because a share of an unknown total is a guess.
struct ContextBar: View {
    let pane: PaneInfo

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Text("Context")
                    .foregroundStyle(.secondary)
                Spacer(minLength: 6)
                Text(detail)
                    .monospacedDigit()
            }
            .font(.caption)

            if let fraction {
                Meter(fraction: fraction, colour: fraction > 0.8 ? .red
                    : (fraction > 0.6 ? .orange : .secondary))
            }
        }
    }

    private var fraction: Double? {
        guard let window = pane.contextWindow, window > 0 else { return nil }
        return Double(pane.tokens.context) / Double(window)
    }

    private var detail: String {
        let used = Readable.tokens(pane.tokens.context)
        guard let window = pane.contextWindow, window > 0 else { return used }
        let percent = Int((Double(pane.tokens.context) / Double(window) * 100).rounded())
        return "\(used) of \(Readable.tokens(window)) · \(percent)%"
    }
}
