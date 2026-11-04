import Foundation

/// Matching an agent's own theme to the otag theme, if the user says so.
///
/// otag's panes already tell the truth about their colours: every pane
/// answers the `OSC 10` and `OSC 11` queries with the active theme's
/// foreground and background, so an agent that asks follows the otag theme
/// with nothing configured and nothing written anywhere. That is the honest
/// mechanism, and it needs no permission.
///
/// This is the one step beyond it. Both agents have a documented theme
/// setting, and setting it makes their accents fit the pane rather than
/// merely sit in it:
///
/// * **Claude Code** — `/theme`, the picker in `/config`, and the `theme`
///   key, valid in any settings file. Built-in presets are `dark`, `light`,
///   `dark-daltonized`, `light-daltonized`, `dark-ansi` and `light-ansi`,
///   plus `custom:<slug>` for a file in `~/.claude/themes/`.
/// * **Codex** — `[tui] theme` in `config.toml`. Its values are *syntax
///   highlighting* theme names, so it recolours code blocks rather than
///   Codex's own interface. Worth setting for that, and worth saying so
///   plainly rather than implying more.
///
/// Nothing here runs without the button being pressed. `plan(for:)` says
/// exactly what would change, for the dialog to show, and `apply(_:)` makes
/// one change per file after writing a timestamped backup. Every path is
/// taken from a `home` the caller supplies, so a test never goes near the
/// real `~/.claude` or `~/.codex`.
public enum AgentTheme {
    // MARK: - What would change

    /// One line otag would write.
    public struct Change: Sendable, Equatable, Identifiable {
        /// Which agent's file.
        public let agent: AgentKind
        /// The file, as a path to show the user.
        public let file: String
        /// The setting, written the way that agent writes it.
        public let key: String
        /// What it says now, or nothing when it says nothing.
        public let from: String?
        /// What it would say.
        public let to: String
        /// One sentence on what this actually affects.
        public let effect: String

        public var id: String { "\(agent.rawValue)-\(key)" }

        /// How the dialog shows it.
        public var line: String {
            "\(key) = \(to)"
        }
    }

    /// Why otag would leave an agent alone.
    public struct Refusal: Sendable, Equatable, Identifiable {
        public let agent: AgentKind
        public let reason: String

        public var id: String { agent.rawValue }
    }

    /// Everything otag would do, and everything it would not.
    public struct Plan: Sendable, Equatable {
        /// The theme the plan matches.
        public let theme: TerminalTheme
        /// The lines otag would write.
        public var changes: [Change] = []
        /// The agents otag would leave alone, and why.
        public var refusals: [Refusal] = []

        /// Whether there is anything to do at all.
        public var isEmpty: Bool { changes.isEmpty }
    }

    /// What otag would change to match `theme`.
    ///
    /// Reads each agent's configuration to say what it says now, so the
    /// dialog can show a before and an after rather than only an after.
    public static func plan(for theme: TerminalTheme, home: URL) -> Plan {
        var plan = Plan(theme: theme)

        // Claude Code.
        let claudeFile = claudeSettings(home: home)
        let current = claudeThemeIn(claudeFile)
        if let current, current.hasPrefix("custom:") {
            plan.refusals.append(Refusal(
                agent: .claude,
                reason: "Claude Code is on your own theme (\(current)). otag leaves a custom "
                    + "theme alone — it is yours, and otag has no idea what is in it."
            ))
        } else {
            let wanted = claudeTheme(for: theme, keeping: current)
            if current == wanted {
                plan.refusals.append(Refusal(
                    agent: .claude,
                    reason: "Claude Code is already on \(wanted)."
                ))
            } else {
                plan.changes.append(Change(
                    agent: .claude,
                    file: "~/.claude/settings.json",
                    key: "theme",
                    from: current,
                    to: "\"\(wanted)\"",
                    effect: "Claude Code's own light or dark theme, which is what colours its "
                        + "diffs, prompts and borders."
                ))
            }
        }

        // Codex.
        let codexFile = codexConfig(home: home)
        let codexCurrent = codexThemeIn(codexFile)
        let codexWanted = codexTheme(for: theme)
        if codexCurrent == codexWanted {
            plan.refusals.append(Refusal(
                agent: .codex,
                reason: "Codex is already on \(codexWanted)."
            ))
        } else {
            plan.changes.append(Change(
                agent: .codex,
                file: "~/.codex/config.toml",
                key: "[tui] theme",
                from: codexCurrent,
                to: "\"\(codexWanted)\"",
                effect: "The theme Codex highlights code blocks with. It does not recolour the "
                    + "rest of its interface — there is no supported setting that does."
            ))
        }

        return plan
    }

    /// Claude Code's preset for an otag theme, keeping whatever variant the
    /// user is already on.
    ///
    /// A `-daltonized` theme is an accessibility choice and a `-ansi` theme
    /// is a deliberate one; switching either to plain `dark` would be otag
    /// quietly taking something away. Only the light-or-dark half changes.
    public static func claudeTheme(for theme: TerminalTheme, keeping current: String?) -> String {
        let half = theme.isDark ? "dark" : "light"
        guard let current else { return half }
        for variant in ["-daltonized", "-ansi"] where current.hasSuffix(variant) {
            return half + variant
        }
        return half
    }

    /// Codex's syntax-highlighting theme for an otag theme.
    ///
    /// One of the names Codex ships. Chosen for the paper each otag theme
    /// uses, which is all a syntax theme can reasonably follow.
    public static func codexTheme(for theme: TerminalTheme) -> String {
        theme.isDark ? "base16-ocean-dark" : "gruvbox-light"
    }

    // MARK: - Doing it

    /// What happened when otag wrote the files.
    public struct Outcome: Sendable, Equatable {
        /// The changes that were written.
        public var written: [Change] = []
        /// What went wrong, for the user to read.
        public var problems: [String] = []
        /// The backups otag kept, as paths.
        public var backups: [String] = []
    }

    /// Carry out a plan: one key per file, with a backup written first.
    ///
    /// Each agent is done on its own, so a file otag cannot write does not
    /// stop the other one.
    public static func apply(_ plan: Plan, home: URL) -> Outcome {
        var outcome = Outcome()
        for change in plan.changes {
            do {
                let backup: String?
                switch change.agent {
                case .claude:
                    backup = try writeClaude(
                        theme: unquoted(change.to),
                        file: claudeSettings(home: home)
                    )
                case .codex:
                    backup = try writeCodex(
                        theme: unquoted(change.to),
                        file: codexConfig(home: home)
                    )
                }
                outcome.written.append(change)
                if let backup { outcome.backups.append(backup) }
            } catch {
                outcome.problems.append(
                    "otag could not change \(change.file): \(error.localizedDescription)"
                )
            }
        }
        return outcome
    }

    // MARK: - The files

    /// Where Claude Code's user settings are.
    public static func claudeSettings(home: URL) -> URL {
        home.appendingPathComponent(".claude/settings.json")
    }

    /// Where Codex's configuration is.
    public static func codexConfig(home: URL) -> URL {
        home.appendingPathComponent(".codex/config.toml")
    }

    /// The theme Claude Code's settings name, if they name one.
    public static func claudeThemeIn(_ file: URL) -> String? {
        guard let data = try? Data(contentsOf: file),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let theme = json["theme"] as? String, !theme.isEmpty
        else { return nil }
        return theme
    }

    /// The theme Codex's configuration names, if it names one.
    ///
    /// A deliberately small reader: the `theme` of the `[tui]` table, and
    /// nothing else about TOML. otag only ever needs to know whether the
    /// value it would write is already there.
    public static func codexThemeIn(_ file: URL) -> String? {
        guard let text = try? String(contentsOf: file, encoding: .utf8) else { return nil }
        var inTui = false
        for raw in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("[") {
                inTui = line == "[tui]"
                continue
            }
            // A dotted key says which table it belongs to by itself.
            if let value = value(of: "tui.theme", in: line) { return value }
            if inTui, let value = value(of: "theme", in: line) { return value }
        }
        return nil
    }

    /// The string a `key = "value"` line assigns, when that is the key.
    private static func value(of key: String, in line: String) -> String? {
        guard line.hasPrefix(key) else { return nil }
        let rest = line.dropFirst(key.count).trimmingCharacters(in: .whitespaces)
        guard rest.hasPrefix("=") else { return nil }
        let text = rest.dropFirst().trimmingCharacters(in: .whitespaces)
        guard text.hasPrefix("\""), text.hasSuffix("\""), text.count >= 2 else { return nil }
        return String(text.dropFirst().dropLast())
    }

    /// Write Claude Code's `theme`, changing nothing else.
    ///
    /// The file is read as JSON, the one key is set, and it is written back.
    /// Key order is not preserved by `JSONSerialization`, so the keys are
    /// sorted: a stable order the user can read beats the order that
    /// happened to be there.
    static func writeClaude(theme: String, file: URL) throws -> String? {
        var settings: [String: Any] = [:]
        var backup: String?
        if let data = try? Data(contentsOf: file) {
            backup = try keepBackup(of: file)
            guard let json = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
                throw AgentThemeError.unreadable(file.lastPathComponent)
            }
            settings = json
        }
        settings["theme"] = theme

        let data = try JSONSerialization.data(
            withJSONObject: settings,
            options: [.prettyPrinted, .sortedKeys]
        )
        try make(directoryFor: file)
        try (String(decoding: data, as: UTF8.self) + "\n").write(
            to: file,
            atomically: true,
            encoding: .utf8
        )
        return backup
    }

    /// Write Codex's `[tui] theme`, changing nothing else.
    ///
    /// The file is TOML and otag is not a TOML editor, so the edit is the
    /// smallest one that is safe: an existing `theme` line inside `[tui]` is
    /// replaced in place, and otherwise the setting is appended as its own
    /// `[tui]` table — or a dotted `tui.theme` line when a `[tui]` table
    /// already exists, which is valid TOML either way. Every other line,
    /// including the comments, is left exactly as it was.
    static func writeCodex(theme: String, file: URL) throws -> String? {
        var backup: String?
        var lines: [String] = []
        var hadFile = false
        if let text = try? String(contentsOf: file, encoding: .utf8) {
            hadFile = true
            backup = try keepBackup(of: file)
            lines = text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        }

        var inTui = false
        var replaced = false
        var hasTuiTable = false
        for index in lines.indices {
            let line = lines[index].trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("[") {
                inTui = line == "[tui]"
                if inTui { hasTuiTable = true }
                continue
            }
            if value(of: "tui.theme", in: line) != nil {
                lines[index] = "tui.theme = \"\(theme)\""
                replaced = true
                break
            }
            if inTui, value(of: "theme", in: line) != nil {
                lines[index] = "theme = \"\(theme)\""
                replaced = true
                break
            }
        }

        if !replaced {
            // A trailing empty line is what a file that ends in a newline
            // splits into; writing after it would leave a blank gap.
            while let last = lines.last, last.trimmingCharacters(in: .whitespaces).isEmpty {
                lines.removeLast()
            }
            if hadFile, !lines.isEmpty { lines.append("") }
            if hasTuiTable {
                lines.append("tui.theme = \"\(theme)\"")
            } else {
                lines.append("[tui]")
                lines.append("theme = \"\(theme)\"")
            }
        }

        try make(directoryFor: file)
        try (lines.joined(separator: "\n") + "\n").write(
            to: file,
            atomically: true,
            encoding: .utf8
        )
        return backup
    }

    /// Keep a copy of a file before changing it.
    ///
    /// Timestamped rather than a single `.bak`, so a second run does not
    /// throw away the original.
    static func keepBackup(of file: URL) throws -> String {
        let stamp = Self.stamp.string(from: Date())
        let copy = file.appendingPathExtension("otag-backup-\(stamp)")
        if FileManager.default.fileExists(atPath: copy.path) {
            try FileManager.default.removeItem(at: copy)
        }
        try FileManager.default.copyItem(at: file, to: copy)
        return copy.path
    }

    /// When a backup was taken, in a form that sorts and has no colons in it.
    static let stamp: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyyMMdd-HHmmss"
        formatter.timeZone = TimeZone(secondsFromGMT: 0)
        return formatter
    }()

    private static func make(directoryFor file: URL) throws {
        try FileManager.default.createDirectory(
            at: file.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
    }

    /// A value as the plan shows it, without the quotes the dialog wants.
    static func unquoted(_ value: String) -> String {
        guard value.hasPrefix("\""), value.hasSuffix("\""), value.count >= 2 else { return value }
        return String(value.dropFirst().dropLast())
    }
}

/// What can go wrong changing an agent's configuration.
public enum AgentThemeError: LocalizedError, Equatable {
    /// The file is there and otag cannot make sense of it.
    case unreadable(String)

    public var errorDescription: String? {
        switch self {
        case let .unreadable(name):
            return "otag could not read \(name), so it changed nothing. Check the file for a "
                + "syntax error and try again."
        }
    }
}
