import Foundation
import Testing

@testable import OtagKit

/// Reading a usage report the way the app has to show it.
///
/// A report is what the agents last said about their windows, from any
/// session on the machine. What matters here is the part the app decides:
/// the line it draws for one window, and what it draws before any agent has
/// said anything at all.
@Suite("How much of the limits is gone")
struct UsageTests {
    /// One window, as the daemon sends it.
    func row(
        window: UsageWindow = .fiveHour,
        tenths: UInt16? = 250,
        resetsAt: Int64? = nil,
        level: UsageLevel = .calm
    ) -> UsageRow {
        UsageRow(
            agent: .claude,
            window: window,
            usedTenths: tenths,
            resetsAt: resetsAt,
            level: level
        )
    }

    func report(_ rows: [UsageRow]) -> UsageReport {
        var report = UsageReport()
        report.rows = rows
        return report
    }

    @Test("a percentage is a whole number, and the meter a fraction of one")
    func aRowReadsAsThePercentageItCarries() {
        #expect(row(tenths: 734).usedPercent == 73)
        #expect(row(tenths: 734).fraction == 0.734)
        #expect(row(tenths: nil).usedPercent == nil, "a window nobody has read")
        #expect(row(tenths: nil).fraction == 0, "and a meter with nothing in it")
    }

    @Test("a report with no reading anywhere has nothing to show")
    func anEmptyReportSaysSo() {
        #expect(report([]).isEmpty)
        #expect(
            report([row(tenths: nil), row(window: .weekly, tenths: nil)]).isEmpty,
            "the windows are there, but no agent has reported one"
        )
        #expect(!report([row()]).isEmpty)
    }

    @Test("a reset within the day is a clock, and one days away carries its date")
    func aResetIsAClockAPersonReads() throws {
        let now = Date(timeIntervalSince1970: 1_793_741_700)
        let soon = try #require(
            Readable.resetClock(Int64(now.timeIntervalSince1970) + 3 * 60 * 60, now: now)
        )
        #expect(!soon.isEmpty)
        let week = try #require(
            Readable.resetClock(Int64(now.timeIntervalSince1970) + 7 * 24 * 60 * 60, now: now)
        )
        #expect(
            week.count > soon.count,
            "a week away needs its date too: \(week) against \(soon)"
        )
        #expect(Readable.resetClock(nil) == nil, "a window with no reset says nothing")
    }

    @Test("a window with no reading at all is a dash")
    func noReadingIsADash() {
        #expect(Readable.noReading == "–")
    }

    @Test("a reading that has not moved is not a second point on the timeline")
    func theTimelineKeepsOneOfEachReading() {
        #expect(
            UsageSample(
                agent: .claude,
                window: .fiveHour,
                usedPercent: 25,
                resetsAt: 1_700_003_600,
                at: 1_700_000_000
            ).date == Date(timeIntervalSince1970: 1_700_000_000)
        )
    }
}
