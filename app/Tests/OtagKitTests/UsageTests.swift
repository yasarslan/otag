import Foundation
import Testing

@testable import OtagKit

/// Reading a usage report the way the app has to show it.
///
/// Usage is account-wide, so a report is a mix of readings from every
/// session on the machine. What matters here is the part the app decides:
/// whether there is anything current to show, and what it says about where
/// the numbers came from.
@Suite("How much of the limits is gone")
struct UsageTests {
    /// One window, as the daemon sends it.
    func row(
        window: UsageWindow = .fiveHour,
        tenths: UInt16? = 250,
        age: Int64 = 0,
        reset: Bool = false,
        session: String? = nil
    ) -> UsageRow {
        var row = UsageRow(
            agent: .claude,
            window: window,
            usedTenths: tenths,
            resetsIn: 3_600,
            fullIn: nil
        )
        row.age = age
        row.reset = reset
        row.session = session
        return row
    }

    func report(_ rows: [UsageRow]) -> UsageReport {
        var report = UsageReport()
        report.rows = rows
        return report
    }

    @Test("a fresh reading is current, an old one and a reset one are not")
    func currentReadings() {
        #expect(row().isCurrent)
        #expect(!row(age: 1_860).isCurrent, "nobody has confirmed this for half an hour")
        #expect(!row(tenths: nil, reset: true).isCurrent)
        #expect(row(age: 600).isCurrent, "ten minutes is still current")
    }

    @Test("a report is fresh when any one window has a current reading")
    func freshness() {
        #expect(report([row()]).isFresh)
        #expect(!report([row(age: 1_860)]).isFresh)
        #expect(!report([]).isFresh)
        #expect(
            report([row(age: 1_860), row(window: .weekly)]).isFresh,
            "one current window is enough to draw something true"
        )
    }

    @Test("the newest reading dates the whole report")
    func newestReading() {
        let newest = report([row(age: 1_800), row(window: .weekly, age: 120)]).newest
        #expect(newest?.window == .weekly)
        #expect(newest?.age == 120)
        #expect(report([]).age == nil)
    }

    @Test("where a number came from is named by the session that reported it")
    func provenanceNamesTheSession() {
        let line = report([row(age: 10_800, session: "a9f87cc5-726a-4466")])
            .provenance(panes: [])
        #expect(line.contains("updated 3h ago"))
        #expect(line.contains("a9f87cc5"), "the identifier is shortened, not shown whole")
        #expect(!line.contains("726a"), "and not the rest of it")
    }

    @Test("a session otag is running is named by what it is working on")
    func provenanceNamesThePane() throws {
        let pane = try JSONDecoder().decode(
            PaneInfo.self,
            from: Data(
                """
                {"id":2,"size":{"rows":24,"cols":80},"workspace":"api",\
                "title":"claude","session":"a9f87cc5-726a"}
                """.utf8
            )
        )
        let line = report([row(session: "a9f87cc5-726a")]).provenance(panes: [pane])
        #expect(line.contains("the session in api"))
        #expect(!line.contains("a9f87cc5"), "a name beats an identifier")
    }

    @Test("a reading from the last minute is not given an age")
    func provenanceOfAFreshReading() {
        let line = report([row(age: 20)]).provenance(panes: [])
        #expect(line.contains("just now"))
        #expect(line.hasSuffix("Click to re-read."))
    }

    @Test("a report with nothing in it says so rather than guessing")
    func provenanceOfNothing() {
        #expect(report([]).provenance(panes: []).contains("No reading yet"))
    }
}
