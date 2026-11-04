import Foundation
import Observation

/// What a session is called.
///
/// Not "claude" and "claude" and "claude". A session is a piece of work, and
/// the thing a person remembers about it is what they asked it to do — so
/// that is its name, taken from its first prompt and editable afterwards.
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
    /// The order is what a person would expect: what they called it, what
    /// they asked it, where it is working, and only then what is running in
    /// it.
    public static func of(
        pane: PaneInfo,
        chosen: String? = nil,
        fromFirstPrompt generated: String? = nil
    ) -> String {
        if let chosen, !chosen.trimmingCharacters(in: .whitespaces).isEmpty {
            return chosen
        }
        if let generated, !generated.isEmpty {
            return generated
        }
        if let folder = folderName(of: pane) {
            return folder
        }
        if !pane.workspace.isEmpty {
            return pane.workspace
        }
        return pane.agent?.label ?? "shell"
    }

    /// The folder a session is working in, by name.
    static func folderName(of pane: PaneInfo) -> String? {
        guard let cwd = pane.cwd, !cwd.isEmpty else { return nil }
        let name = URL(fileURLWithPath: cwd, isDirectory: true).lastPathComponent
        return name.isEmpty || name == "/" ? nil : name
    }

    /// The first line of a prompt that is a sentence rather than scaffolding.
    ///
    /// A fenced code block, a line that is only a path, and a line that is
    /// only punctuation are all skipped: they are what the prompt is *about*,
    /// not what it asks for.
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
        let extension_ = line[line.index(after: dot)...]
        return !extension_.isEmpty
            && extension_.count <= 5
            && extension_.allSatisfy(\.isLetter)
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

/// The names the user has given, and the ones otag made from first prompts.
///
/// Kept by the app rather than the daemon: a name is a label on a window,
/// and the daemon's business is what is running. Written to `UserDefaults`
/// so a restart does not forget them, and pruned to the sessions the daemon
/// still reports so the file does not grow forever.
///
/// **Known gap**: otag can only generate a name from a prompt it sent itself,
/// which means from the new-session sheet. A session started from the
/// terminal client is named after its folder until it is renamed. Naming
/// those too needs the daemon to report a session's first prompt.
@MainActor
@Observable
public final class SessionNames {
    /// Where the names are kept.
    public static let chosenKey = "sessionNames"
    public static let generatedKey = "sessionNamesFromPrompts"

    /// What the user called each session.
    private var chosen: [PaneID: String] = [:]
    /// What otag made from each session's first prompt.
    private var generated: [PaneID: String] = [:]

    private let defaults: UserDefaults

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        chosen = Self.read(Self.chosenKey, from: defaults)
        generated = Self.read(Self.generatedKey, from: defaults)
    }

    /// What this session is called.
    public func name(of pane: PaneInfo) -> String {
        SessionName.of(pane: pane, chosen: chosen[pane.id], fromFirstPrompt: generated[pane.id])
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

    /// Remember what a session was first asked, as its name.
    ///
    /// Only the first prompt counts, and only when the user has not named
    /// the session themselves.
    public func remember(firstPrompt prompt: String, for pane: PaneID) {
        guard generated[pane] == nil, let name = SessionName.from(prompt: prompt) else { return }
        generated[pane] = name
        write()
    }

    /// Forget the sessions the daemon no longer runs.
    public func prune(to alive: [PaneID]) {
        let running = Set(alive)
        let before = chosen.count + generated.count
        chosen = chosen.filter { running.contains($0.key) }
        generated = generated.filter { running.contains($0.key) }
        if chosen.count + generated.count != before { write() }
    }

    private func write() {
        defaults.set(Self.encode(chosen), forKey: Self.chosenKey)
        defaults.set(Self.encode(generated), forKey: Self.generatedKey)
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
