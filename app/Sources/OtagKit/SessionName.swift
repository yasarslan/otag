import Foundation
import Observation

/// What a session is called.
///
/// Not "claude" and "claude" and "claude", and not the folder three times
/// over: every session in one project was named after that project, which is
/// the same as having no names at all. A session is a piece of work, and the
/// thing a person remembers about it is what they asked it to do.
///
/// The rules below are deliberately dull. A name is a label in a list, not a
/// summary: cutting a prompt down to its first clause is the whole trick, and
/// anything cleverer would be otag guessing at meaning it does not have.
public enum SessionName {
    /// How long a generated name may be.
    public static let limit = 40

    /// Openings that say nothing about the work.
    ///
    /// Longest first, so "can you please" loses both words rather than one.
    static let filler = [
        "i want you to", "i would like you to", "i'd like you to",
        "can you please", "could you please", "would you please",
        "please can you", "please could you",
        "can you", "could you", "would you", "will you",
        "help me", "let us", "let's", "now", "please", "hey", "ok", "okay",
    ]

    /// A name made from a session's first prompt, or nothing when the prompt
    /// has nothing in it worth a name.
    public static func from(prompt: String) -> String? {
        guard let line = firstUsefulLine(of: prompt) else { return nil }

        var words = stripFiller(from: line)
        guard !words.isEmpty else { return nil }

        words = cut(words, to: limit)
        guard !words.isEmpty else { return nil }

        return sentenceCased(words)
    }

    /// A name for a session, from everything otag knows about it.
    ///
    /// An explicit rename wins. Team roles use their configured names.
    /// Other sessions use the first prompt or a numbered agent label.
    ///
    /// The folder is deliberately *not* a source. It is in the sidebar's
    /// section header and in the session's own header bar already, and using
    /// it as a name is what made every session in a project look identical.
    public static func of(
        pane: PaneInfo,
        chosen: String? = nil,
        ordinal: Int? = nil
    ) -> String {
        if let chosen, !chosen.trimmingCharacters(in: .whitespaces).isEmpty {
            return chosen
        }
        if let team = pane.team {
            if let role = team.roleName, !role.isEmpty { return role }
            if !pane.title.isEmpty { return pane.title }
            return fallback(for: pane, ordinal: ordinal)
        }
        if let prompt = pane.firstPrompt, let name = from(prompt: prompt) {
            return name
        }
        return fallback(for: pane, ordinal: ordinal)
    }

    /// What a session with nothing asked of it yet is called: the agent, and
    /// which one of them it is.
    public static func fallback(for pane: PaneInfo, ordinal: Int?) -> String {
        let what = pane.agent?.label ?? "Shell"
        guard let ordinal else { return what }
        return "\(what) \(ordinal)"
    }

    /// Which one of its agent's sessions this is, counting in the order the
    /// daemon lists them.
    ///
    /// One-based, so the first Codex session is "Codex 1". Counting only
    /// within an agent is what makes the numbers stable while the other
    /// agent's sessions come and go.
    public static func ordinals(of panes: [PaneInfo]) -> [PaneID: Int] {
        var counts: [String: Int] = [:]
        var ordinals: [PaneID: Int] = [:]
        for pane in panes {
            let key = pane.agent?.rawValue ?? ""
            let next = (counts[key] ?? 0) + 1
            counts[key] = next
            ordinals[pane.id] = next
        }
        return ordinals
    }

    /// The first line of a prompt that is a sentence rather than scaffolding.
    ///
    /// A fenced code block, a line that is only a path, and a line that is
    /// only punctuation are all skipped: they are what the prompt is *about*,
    /// not what it asks for. The daemon already drops a whole prompt that is
    /// injected context; this is the rest of the tidying.
    private static func firstUsefulLine(of prompt: String) -> String? {
        var fenced = false
        for raw in prompt.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("```") {
                fenced.toggle()
                continue
            }
            if fenced || line.isEmpty { continue }
            if isPathOnly(line) { continue }
            if line.rangeOfCharacter(from: .alphanumerics) == nil { continue }
            return line
        }
        return nil
    }

    /// Whether a line is nothing but a file path, which names no work.
    ///
    /// One word with a directory separator or a file extension in it: a
    /// prompt that opens with `crates/otag-core/src/session.rs` is saying
    /// what it is *about*, and the ask is on the next line.
    private static func isPathOnly(_ line: String) -> Bool {
        guard line.rangeOfCharacter(from: .whitespaces) == nil else { return false }
        if line.contains("/") { return true }
        // A trailing `.rs`, `.md`, `.toml`: a short run of letters after a
        // dot, with something in front of it.
        guard let dot = line.lastIndex(of: "."), dot != line.startIndex else { return false }
        let suffix = line[line.index(after: dot)...]
        return !suffix.isEmpty && suffix.count <= 5 && suffix.allSatisfy(\.isLetter)
    }

    /// The line with its empty opening removed, as words.
    private static func stripFiller(from line: String) -> [String] {
        // Markdown headings and list bullets are punctuation around the ask.
        var text = line
        while let first = text.first, "#>-*•".contains(first) {
            text.removeFirst()
            text = text.trimmingCharacters(in: .whitespaces)
        }

        var lowered = text.lowercased()
        var removing = true
        while removing {
            removing = false
            for opening in filler where lowered.hasPrefix(opening) {
                let after = lowered.index(lowered.startIndex, offsetBy: opening.count)
                // Only a whole word: "please" must not eat into "pleased".
                guard after == lowered.endIndex || !lowered[after].isLetter else { continue }
                text = String(text[after...])
                text = text.trimmingCharacters(in: CharacterSet(charactersIn: " \t,:"))
                lowered = text.lowercased()
                removing = true
                break
            }
        }

        return text
            .split(whereSeparator: \.isWhitespace)
            .map(String.init)
    }

    /// As many whole words as fit, and never a cut-off one.
    private static func cut(_ words: [String], to limit: Int) -> [String] {
        var kept: [String] = []
        var length = 0
        for word in words {
            let added = kept.isEmpty ? word.count : word.count + 1
            if length + added > limit {
                // One very long first word is still better than no name;
                // the row truncates it if it must.
                if kept.isEmpty { kept = [String(word.prefix(limit))] }
                break
            }
            kept.append(word)
            length += added
        }
        return kept
    }

    /// A sentence, not a heading: the first letter up, the rest left alone,
    /// and no trailing punctuation.
    ///
    /// The rest is left alone on purpose — `CLAUDE.md`, `PaneID` and `gRPC`
    /// are how they are written, and a name that lower-cases them is wrong
    /// in a way a person notices immediately.
    private static func sentenceCased(_ words: [String]) -> String? {
        var text = words.joined(separator: " ")
        while let last = text.last, ".,;:!?-–—".contains(last) {
            text.removeLast()
        }
        text = text.trimmingCharacters(in: .whitespaces)
        guard let first = text.first else { return nil }
        // A word that is already capitalised, or shouting, is left as it is.
        guard first.isLowercase else { return text }
        return text.replacingCharacters(
            in: text.startIndex ... text.startIndex,
            with: String(first).uppercased()
        )
    }
}

/// The names the user has given their sessions.
///
/// Nicknames are *not* here: the daemon keeps those, because they have to
/// survive a restart and reach every client. These are the renames, which
/// only this window cares about.
///
/// Only the renames: a session's own name comes from the first prompt, which
/// the daemon reports, so there is nothing for the app to remember about it.
/// Kept in `UserDefaults` so a restart does not forget a rename, and pruned
/// to the sessions the daemon still reports so the file does not grow
/// forever.
@MainActor
@Observable
public final class SessionNames {
    /// Where the names are kept.
    public static let chosenKey = "sessionNames"

    /// What the user called each session.
    private var chosen: [PaneID: String] = [:]
    /// Which one of its agent's sessions each pane is, from the last report.
    private var ordinals: [PaneID: Int] = [:]

    private let defaults: UserDefaults

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        chosen = Self.read(Self.chosenKey, from: defaults)
    }

    /// Take in the daemon's list, which is what the numbering counts along.
    ///
    /// Also forgets the renames of sessions the daemon no longer runs.
    public func took(_ panes: [PaneInfo]) {
        ordinals = SessionName.ordinals(of: panes)
        prune(to: panes.map(\.id))
    }

    /// What this session is called.
    public func name(of pane: PaneInfo) -> String {
        SessionName.of(pane: pane, chosen: chosen[pane.id], ordinal: ordinals[pane.id])
    }

    /// Whether the user has named this session themselves.
    public func isRenamed(_ pane: PaneID) -> Bool {
        chosen[pane] != nil
    }

    /// Call this session something else.
    ///
    /// An empty name is not a name: it puts the session back to whatever
    /// otag would have called it, which is how Rename cancels itself.
    public func rename(_ pane: PaneID, to name: String) {
        let wanted = name.trimmingCharacters(in: .whitespacesAndNewlines)
        if wanted.isEmpty {
            chosen.removeValue(forKey: pane)
        } else {
            chosen[pane] = String(wanted.prefix(80))
        }
        write()
    }

    /// Forget the sessions the daemon no longer runs.
    public func prune(to alive: [PaneID]) {
        let running = Set(alive)
        let before = chosen.count
        chosen = chosen.filter { running.contains($0.key) }
        if chosen.count != before { write() }
    }

    private func write() {
        defaults.set(Self.encode(chosen), forKey: Self.chosenKey)
    }

    /// `PaneID` is a number, and `UserDefaults` wants string keys.
    private static func encode(_ names: [PaneID: String]) -> [String: String] {
        Dictionary(uniqueKeysWithValues: names.map { ("\($0.key)", $0.value) })
    }

    private static func read(_ key: String, from defaults: UserDefaults) -> [PaneID: String] {
        guard let stored = defaults.dictionary(forKey: key) as? [String: String] else { return [:] }
        var names: [PaneID: String] = [:]
        for (id, name) in stored {
            guard let pane = PaneID(id) else { continue }
            names[pane] = name
        }
        return names
    }
}
