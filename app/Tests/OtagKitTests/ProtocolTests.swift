import Foundation
import Testing

@testable import OtagKit

/// The daemon's own tests cover what it sends; these cover that the app reads
/// exactly that, including the parts that are allowed to be missing.
@Suite("The socket protocol")
struct ProtocolTests {
    /// One request as the line the daemon would receive.
    private func line(_ request: Request) throws -> String {
        let data = try Wire.encode(request)
        #expect(data.last == 0x0A, "every request is one line")
        return String(decoding: data.dropLast(), as: UTF8.self)
    }

    /// One response from a line the daemon would send.
    private func response(_ json: String) throws -> Response {
        try Wire.decode(Data(json.utf8))
    }

    @Test("a request carries its type and its own fields")
    func requestsEncode() throws {
        #expect(try line(.ping) == #"{"type":"ping"}"#)
        #expect(try line(.subscribe) == #"{"type":"subscribe"}"#)

        let attach = try line(.attachPane(pane: 3, size: PaneSize(rows: 24, cols: 80)))
        #expect(attach.contains(#""type":"attach_pane""#))
        #expect(attach.contains(#""pane":3"#))
        #expect(attach.contains(#""rows":24"#))
        #expect(attach.contains(#""cols":80"#))

        // A pane whose size the app is not claiming leaves the field out, so
        // the daemon's layout keeps deciding.
        let without = try line(.attachPane(pane: 3, size: nil))
        #expect(!without.contains("size"))
    }

    @Test("input travels as base64, which is what the daemon reads")
    func inputEncodesAsBase64() throws {
        let encoded = try line(.paneInput(pane: 1, bytes: Data([0x1B, 0x5B, 0x41])))
        #expect(encoded.contains(#""bytes":"G1tB""#))
    }

    @Test("an answer names the question and what to do")
    func answerEncodes() throws {
        let encoded = try line(.answer(ask: 7, answer: .deny))
        #expect(encoded.contains(#""type":"answer""#))
        #expect(encoded.contains(#""ask":7"#))
        #expect(encoded.contains(#""answer":"deny""#))
    }

    @Test("a pane's screen arrives with its size and its bytes")
    func paneScreenDecodes() throws {
        let decoded = try response(
            #"{"type":"pane_screen","pane":2,"size":{"rows":10,"cols":20},"bytes":"G1tB"}"#
        )
        guard case let .paneScreen(pane, size, bytes) = decoded else {
            Issue.record("expected a screen, got \(decoded)")
            return
        }
        #expect(pane == 2)
        #expect(size == PaneSize(rows: 10, cols: 20))
        #expect(Array(bytes) == [0x1B, 0x5B, 0x41])
    }

    @Test("output arrives as bytes, not as text")
    func outputDecodes() throws {
        let decoded = try response(#"{"type":"output","pane":1,"bytes":"aGk="}"#)
        guard case let .output(pane, bytes) = decoded else {
            Issue.record("expected output, got \(decoded)")
            return
        }
        #expect(pane == 1)
        #expect(String(decoding: bytes, as: UTF8.self) == "hi")
    }

    @Test("a pane is read even when the daemon leaves most of it out")
    func paneInfoFillsInWhatIsMissing() throws {
        let decoded = try response(
            #"{"type":"panes","panes":[{"id":1,"size":{"rows":24,"cols":80}}]}"#
        )
        guard case let .panes(panes) = decoded, let pane = panes.first else {
            Issue.record("expected one pane, got \(decoded)")
            return
        }
        #expect(pane.id == 1)
        #expect(pane.status == .idle)
        #expect(pane.agent == nil)
        #expect(pane.title.isEmpty)
        #expect(pane.tokens.context == 0)
    }

    @Test("an agent or a status this build does not know about is not fatal")
    func unknownValuesDoNotBreakAPane() throws {
        let decoded = try response(
            """
            {"type":"panes","panes":[{"id":4,"size":{"rows":2,"cols":2},\
            "agent":"invented_later","status":"invented_later","title":"x"}]}
            """
        )
        guard case let .panes(panes) = decoded, let pane = panes.first else {
            Issue.record("expected one pane, got \(decoded)")
            return
        }
        #expect(pane.agent == nil)
        #expect(pane.status == .idle)
        #expect(pane.title == "x")
    }

    @Test("an event carries the whole state it describes")
    func eventsDecode() throws {
        let panes = try response(
            #"{"type":"event","event":{"event":"panes","panes":[]}}"#
        )
        guard case let .event(.panes(list)) = panes else {
            Issue.record("expected a panes event, got \(panes)")
            return
        }
        #expect(list.isEmpty)

        let asked = try response(
            """
            {"type":"event","event":{"event":"asked","ask":{"id":9,"pane":2,\
            "agent":"claude","tool":"Bash","summary":"Bash: ls","asked_at":1700000000}}}
            """
        )
        guard case let .event(.asked(ask)) = asked else {
            Issue.record("expected an asked event, got \(asked)")
            return
        }
        #expect(ask.id == 9)
        #expect(ask.pane == 2)
        #expect(ask.agent == .claude)
        #expect(ask.summary == "Bash: ls")
    }

    @Test("a message or event from a newer daemon is named, not refused")
    func unknownMessagesDecode() throws {
        let message = try response(#"{"type":"invented_later","whatever":1}"#)
        guard case let .unknown(kind) = message else {
            Issue.record("expected an unknown message, got \(message)")
            return
        }
        #expect(kind == "invented_later")

        let event = try response(#"{"type":"event","event":{"event":"invented_later"}}"#)
        guard case .event(.unknown) = event else {
            Issue.record("expected an unknown event, got \(event)")
            return
        }
    }

    @Test("usage is read in the tenths the daemon sends")
    func usageDecodes() throws {
        let decoded = try response(
            """
            {"type":"usage","report":{"rows":[{"agent":"claude","window":"five_hour",\
            "used_tenths":734,"resets_in":3600,"level":"amber","at":1700000000}],\
            "counted":[{"agent":"codex","tokens":1234}]}}
            """
        )
        guard case let .usage(report) = decoded, let row = report.rows.first else {
            Issue.record("expected a usage report, got \(decoded)")
            return
        }
        #expect(row.agent == .claude)
        #expect(row.window == .fiveHour)
        #expect(row.usedPercent == 73)
        #expect(row.resetsIn == 3_600)
        #expect(row.fullIn == nil)
        #expect(row.level == .amber)
        #expect(report.counted.first?.tokens == 1_234)
    }

    @Test("the readings behind the numbers are read for the chart")
    func historyDecodes() throws {
        let decoded = try response(
            """
            {"type":"usage_history","samples":[{"agent":"claude","window":"five_hour",\
            "used_pct":73.4,"resets_at":1700003600,"at":1700000000}]}
            """
        )
        guard case let .usageHistory(samples) = decoded, let sample = samples.first else {
            Issue.record("expected a history, got \(decoded)")
            return
        }
        #expect(sample.agent == .claude)
        #expect(sample.window == .fiveHour)
        #expect(abs(sample.usedPercent - 73.4) < 0.001)
        #expect(sample.at == 1_700_000_000)
        #expect(sample.date == Date(timeIntervalSince1970: 1_700_000_000))
    }

    @Test("a pane carries how large its model's context is, when otag knows")
    func contextWindowDecodes() throws {
        let decoded = try response(
            """
            {"type":"panes","panes":[{"id":1,"size":{"rows":2,"cols":2},\
            "tokens":{"input":1,"output":2,"cache_read":3,"cache_written":4,\
            "context":50000},"context_window":200000}]}
            """
        )
        guard case let .panes(panes) = decoded, let pane = panes.first else {
            Issue.record("expected one pane, got \(decoded)")
            return
        }
        #expect(pane.tokens.context == 50_000)
        #expect(pane.contextWindow == 200_000)

        // A pane whose window otag does not know has none, rather than a
        // guessed one.
        let without = try response(
            #"{"type":"panes","panes":[{"id":1,"size":{"rows":2,"cols":2}}]}"#
        )
        guard case let .panes(bare) = without else {
            Issue.record("expected one pane")
            return
        }
        #expect(bare.first?.contextWindow == nil)
    }

    @Test("statuses are ordered by how much they want the user")
    func statusOrdering() {
        #expect(PaneStatus.needsApproval > PaneStatus.error)
        #expect(PaneStatus.error > PaneStatus.looping)
        #expect(PaneStatus.working > PaneStatus.idle)
        #expect([PaneStatus.idle, .needsApproval, .working].max() == .needsApproval)
        #expect(PaneStatus.needsApproval.wantsAttention)
        #expect(!PaneStatus.done.wantsAttention)
    }
}
