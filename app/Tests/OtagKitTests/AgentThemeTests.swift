import Foundation
import Testing

@testable import OtagKit

/// Changing an agent's own theme, once the user has said so.
///
/// This is the only code in otag that writes to a file of the user's outside
/// otag itself, so it is the code that most needs pinning down: one key per
/// file, a backup first, an accessibility choice preserved, and a custom
/// theme never touched.
///
/// Every test works in a temporary home. The real `~/.claude` and `~/.codex`
/// are never read from or written to here.
@Suite("Matching the agents' themes")
struct AgentThemeTests {
    /// A home of its own, cleaned up afterwards.
    private func home() throws -> URL {
        let url = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            .appendingPathComponent("otag-agent-theme-\(UUID())", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    private func write(_ text: String, to file: URL) throws {
        try FileManager.default.createDirectory(
            at: file.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try text.write(to: file, atomically: true, encoding: .utf8)
    }

    // MARK: - What would change

    @Test("a dark otag theme means a dark agent theme, and a light one light")
    func theObviousCase() throws {
        let home = try home()
        defer { try? FileManager.default.removeItem(at: home) }

        let dark = AgentTheme.plan(for: .glassDark, home: home)
        #expect(dark.changes.count == 2, "both agents have something to set")
        #expect(dark.changes.first { $0.agent == .claude }?.to == "\"dark\"")
        #expect(dark.changes.first { $0.agent == .codex }?.to == "\"base16-ocean-dark\"")

        let light = AgentTheme.plan(for: .glassLight, home: home)
        #expect(light.changes.first { $0.agent == .claude }?.to == "\"light\"")
        #expect(light.changes.first { $0.agent == .codex }?.to == "\"gruvbox-light\"")

        // Midnight is a dark look, whatever it is called.
        let midnight = AgentTheme.plan(for: .midnight, home: home)
        #expect(midnight.changes.first { $0.agent == .claude }?.to == "\"dark\"")
    }

    @Test("an accessibility variant is kept, never swapped for the plain one")
    func daltonizedIsPreserved() {
        // Someone on light-daltonized wants the daltonized colours; otag
        // changing only the light-or-dark half is the whole point.
        #expect(
            AgentTheme.claudeTheme(for: .glassDark, keeping: "light-daltonized")
                == "dark-daltonized"
        )
        #expect(
            AgentTheme.claudeTheme(for: .glassLight, keeping: "dark-daltonized")
                == "light-daltonized"
        )
        #expect(AgentTheme.claudeTheme(for: .glassDark, keeping: "light-ansi") == "dark-ansi")
        #expect(AgentTheme.claudeTheme(for: .glassLight, keeping: "dark-ansi") == "light-ansi")

        // And the plain presets stay plain.
        #expect(AgentTheme.claudeTheme(for: .glassDark, keeping: "light") == "dark")
        #expect(AgentTheme.claudeTheme(for: .glassDark, keeping: nil) == "dark")
    }

    @Test("a plan reads what the files say now, so the dialog can show a before")
    func theBefore() throws {
        let home = try home()
        defer { try? FileManager.default.removeItem(at: home) }

        try write(
            #"{"theme": "light-daltonized", "model": "opus"}"#,
            to: AgentTheme.claudeSettings(home: home)
        )
        try write("[tui]\ntheme = \"zenburn\"\n", to: AgentTheme.codexConfig(home: home))

        let plan = AgentTheme.plan(for: .midnight, home: home)
        let claude = try #require(plan.changes.first { $0.agent == .claude })
        #expect(claude.from == "light-daltonized")
        #expect(claude.to == "\"dark-daltonized\"")

        let codex = try #require(plan.changes.first { $0.agent == .codex })
        #expect(codex.from == "zenburn")
    }

    @Test("a custom Claude Code theme is left alone, and otag says why")
    func aCustomThemeIsRefused() throws {
        let home = try home()
        defer { try? FileManager.default.removeItem(at: home) }

        try write(#"{"theme": "custom:dracula"}"#, to: AgentTheme.claudeSettings(home: home))

        let plan = AgentTheme.plan(for: .glassDark, home: home)
        #expect(!plan.changes.contains { $0.agent == .claude })
        let refusal = try #require(plan.refusals.first { $0.agent == .claude })
        #expect(refusal.reason.contains("custom:dracula"))

        // And applying it does not write the file either.
        let before = try String(contentsOf: AgentTheme.claudeSettings(home: home), encoding: .utf8)
        _ = AgentTheme.apply(plan, home: home)
        let after = try String(contentsOf: AgentTheme.claudeSettings(home: home), encoding: .utf8)
        #expect(before == after)
    }

    @Test("an agent already on the right theme has nothing to change")
    func alreadyRight() throws {
        let home = try home()
        defer { try? FileManager.default.removeItem(at: home) }

        try write(#"{"theme": "dark"}"#, to: AgentTheme.claudeSettings(home: home))
        try write(
            "[tui]\ntheme = \"base16-ocean-dark\"\n",
            to: AgentTheme.codexConfig(home: home)
        )

        let plan = AgentTheme.plan(for: .glassDark, home: home)
        #expect(plan.isEmpty, "there is nothing to do")
        #expect(plan.refusals.count == 2)
        for refusal in plan.refusals {
            #expect(refusal.reason.contains("already"))
        }
    }

    // MARK: - Doing it

    @Test("applying a plan changes one key and keeps everything else")
    func claudeIsEditedSurgically() throws {
        let home = try home()
        defer { try? FileManager.default.removeItem(at: home) }

        let file = AgentTheme.claudeSettings(home: home)
        try write(
            #"{"theme": "light", "model": "opus", "permissions": {"allow": ["Bash(ls)"]}}"#,
            to: file
        )

        let outcome = AgentTheme.apply(AgentTheme.plan(for: .glassDark, home: home), home: home)
        #expect(outcome.problems.isEmpty, "\(outcome.problems)")

        let after = try #require(
            try JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [String: Any]
        )
        #expect(after["theme"] as? String == "dark")
        #expect(after["model"] as? String == "opus", "the rest of the file is untouched")
        let permissions = try #require(after["permissions"] as? [String: Any])
        #expect(permissions["allow"] as? [String] == ["Bash(ls)"])
    }

    @Test("a backup is kept before anything is written")
    func backupsAreKept() throws {
        let home = try home()
        defer { try? FileManager.default.removeItem(at: home) }

        let claudeFile = AgentTheme.claudeSettings(home: home)
        let codexFile = AgentTheme.codexConfig(home: home)
        try write(#"{"theme": "light"}"#, to: claudeFile)
        try write("model = \"gpt\"\n", to: codexFile)

        let outcome = AgentTheme.apply(AgentTheme.plan(for: .glassDark, home: home), home: home)
        #expect(outcome.written.count == 2)
        #expect(outcome.backups.count == 2, "one copy per file changed")

        for path in outcome.backups {
            #expect(path.contains("otag-backup-"))
            #expect(FileManager.default.fileExists(atPath: path))
        }
        // The copy is what was there before.
        let claudeBackup = try #require(outcome.backups.first { $0.contains("settings.json") })
        #expect(
            try String(contentsOfFile: claudeBackup, encoding: .utf8)
                .contains("light")
        )
    }

    @Test("a file otag has never seen is created rather than refused")
    func noFileYet() throws {
        let home = try home()
        defer { try? FileManager.default.removeItem(at: home) }

        let outcome = AgentTheme.apply(AgentTheme.plan(for: .midnight, home: home), home: home)
        #expect(outcome.problems.isEmpty, "\(outcome.problems)")
        #expect(outcome.backups.isEmpty, "there was nothing to back up")

        #expect(
            AgentTheme.claudeThemeIn(AgentTheme.claudeSettings(home: home)) == "dark"
        )
        #expect(
            AgentTheme.codexThemeIn(AgentTheme.codexConfig(home: home)) == "base16-ocean-dark"
        )
    }

    @Test("Codex's own lines, comments and all, are left as they were")
    func codexIsEditedInPlace() throws {
        let home = try home()
        defer { try? FileManager.default.removeItem(at: home) }

        let file = AgentTheme.codexConfig(home: home)
        try write(
            """
            # my settings
            model = "gpt"

            [tui]
            # the look
            theme = "zenburn"
            mouse_scroll_speed = 3

            [mcp_servers.thing]
            command = "run"
            """,
            to: file
        )

        _ = AgentTheme.apply(AgentTheme.plan(for: .glassLight, home: home), home: home)

        let after = try String(contentsOf: file, encoding: .utf8)
        #expect(after.contains("theme = \"gruvbox-light\""))
        #expect(!after.contains("zenburn"))
        #expect(after.contains("# my settings"), "the comments are still there")
        #expect(after.contains("# the look"))
        #expect(after.contains("mouse_scroll_speed = 3"))
        #expect(after.contains("[mcp_servers.thing]"))
        #expect(after.contains("model = \"gpt\""))
    }

    @Test("a config with no [tui] table gains one rather than a mangled line")
    func codexWithoutATuiTable() throws {
        let home = try home()
        defer { try? FileManager.default.removeItem(at: home) }

        let file = AgentTheme.codexConfig(home: home)
        try write("model = \"gpt\"\n", to: file)

        _ = AgentTheme.apply(AgentTheme.plan(for: .glassDark, home: home), home: home)

        let after = try String(contentsOf: file, encoding: .utf8)
        #expect(after.contains("[tui]"))
        #expect(AgentTheme.codexThemeIn(file) == "base16-ocean-dark")
        // The table must come after the top-level keys, or those keys would
        // silently become part of it.
        let table = try #require(after.range(of: "[tui]"))
        let model = try #require(after.range(of: "model = "))
        #expect(model.lowerBound < table.lowerBound)
    }

    @Test("a dotted tui.theme line is read and replaced where it is")
    func codexDottedKey() throws {
        let home = try home()
        defer { try? FileManager.default.removeItem(at: home) }

        let file = AgentTheme.codexConfig(home: home)
        try write("tui.theme = \"zenburn\"\nmodel = \"gpt\"\n", to: file)
        #expect(AgentTheme.codexThemeIn(file) == "zenburn")

        _ = AgentTheme.apply(AgentTheme.plan(for: .glassDark, home: home), home: home)
        let after = try String(contentsOf: file, encoding: .utf8)
        #expect(after.contains("tui.theme = \"base16-ocean-dark\""))
        #expect(!after.contains("[tui]"), "it did not add a table it does not need")
    }

    @Test("a theme key in some other table is not mistaken for Codex's")
    func codexOnlyReadsItsOwnTable() throws {
        let home = try home()
        defer { try? FileManager.default.removeItem(at: home) }

        let file = AgentTheme.codexConfig(home: home)
        try write("[something_else]\ntheme = \"not-ours\"\n", to: file)
        #expect(AgentTheme.codexThemeIn(file) == nil)

        _ = AgentTheme.apply(AgentTheme.plan(for: .glassDark, home: home), home: home)
        let after = try String(contentsOf: file, encoding: .utf8)
        #expect(after.contains("theme = \"not-ours\""), "somebody else's key is untouched")
        #expect(after.contains("[tui]"))
    }

    @Test("a settings file otag cannot parse is reported, not overwritten")
    func brokenSettings() throws {
        let home = try home()
        defer { try? FileManager.default.removeItem(at: home) }

        let file = AgentTheme.claudeSettings(home: home)
        try write("{ this is not json", to: file)

        let outcome = AgentTheme.apply(AgentTheme.plan(for: .glassDark, home: home), home: home)
        #expect(outcome.problems.count == 1)
        #expect(outcome.problems[0].contains("~/.claude/settings.json"))
        #expect(
            try String(contentsOf: file, encoding: .utf8) == "{ this is not json",
            "the file otag could not read is the file otag did not write"
        )
        // Codex is a different file and is not held up by it.
        #expect(outcome.written.map(\.agent) == [.codex])
    }

    @Test("running it twice changes nothing the second time")
    func idempotent() throws {
        let home = try home()
        defer { try? FileManager.default.removeItem(at: home) }

        _ = AgentTheme.apply(AgentTheme.plan(for: .glassDark, home: home), home: home)
        let after = try String(
            contentsOf: AgentTheme.codexConfig(home: home),
            encoding: .utf8
        )

        let again = AgentTheme.plan(for: .glassDark, home: home)
        #expect(again.isEmpty, "there is nothing left to do")
        _ = AgentTheme.apply(again, home: home)
        #expect(
            try String(contentsOf: AgentTheme.codexConfig(home: home), encoding: .utf8) == after
        )
    }

    @Test("every change says plainly what it affects")
    func everyChangeExplainsItself() throws {
        let home = try home()
        defer { try? FileManager.default.removeItem(at: home) }

        let plan = AgentTheme.plan(for: .glassDark, home: home)
        for change in plan.changes {
            #expect(!change.effect.isEmpty, "\(change.agent) does not say what it changes")
            #expect(change.file.hasPrefix("~/"), "the dialog shows a path a person recognises")
            #expect(!change.line.isEmpty)
        }
        // Codex's line must not over-claim: it is syntax highlighting.
        let codex = try #require(plan.changes.first { $0.agent == .codex })
        #expect(codex.effect.lowercased().contains("code block"))
    }
}
