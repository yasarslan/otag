import OtagKit
import SwiftUI

/// Where a session's work can go: the other agent, or another model.
///
/// Two honest answers when an agent runs out of road, and they are not the
/// same thing. Handing over carries a package the other agent reads and
/// starts a new conversation there. Changing the model keeps the
/// conversation and changes only what is answering it.
struct SwitchAgentPopover: View {
    let actions: SessionActions

    @State private var model = ""

    var body: some View {
        VStack(alignment: .leading, spacing: Chrome.gutter) {
            VStack(alignment: .leading, spacing: 2) {
                Text(Strings.t("switch.title"))
                    .font(.headline)
                Text(actions.oneLine.isEmpty ? actions.name : actions.oneLine)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            Divider()

            VStack(alignment: .leading, spacing: 6) {
                Button(handOff) { actions.handOff() }
                    .primaryActionStyle(prominent: actions.isRunningOut)
                    .disabled(!actions.canHandOff)
                Text(reason)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Divider()

            VStack(alignment: .leading, spacing: 6) {
                Text(Strings.t("switch.model"))
                    .font(.subheadline.weight(.semibold))
                Text(Strings.t("switch.model.detail"))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)

                ForEach(AgentModels.of(actions.pane.agent), id: \.self) { name in
                    Button(name) { actions.continueOn(model: name) }
                        .buttonStyle(.link)
                }

                HStack(spacing: 6) {
                    TextField(Strings.t("switch.model.other"), text: $model)
                        .textFieldStyle(.roundedBorder)
                    Button(Strings.t("switch.model.use")) { actions.continueOn(model: model) }
                        .disabled(model.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                .padding(.top, 2)
            }
            .disabled(actions.pane.agent == nil)
        }
        .padding(Chrome.gutter)
        .frame(width: 340, alignment: .leading)
    }

    /// What the handoff button says, named after where the work would go.
    private var handOff: String {
        switch actions.pane.agent {
        case .claude: return Strings.t("switch.handoff.codex")
        case .codex: return Strings.t("switch.handoff.claude")
        case nil: return Strings.t("switch.handoff.none")
        }
    }

    /// Why the handoff is or is not available, in one line.
    private var reason: String {
        if actions.pane.agent == nil {
            return Strings.t("switch.handoff.shell")
        }
        if actions.isHandingOff {
            return Strings.t("switch.handoff.busy")
        }
        if actions.isRunningOut {
            return Strings.t("switch.handoff.urgent")
        }
        return Strings.t("switch.handoff.detail")
    }
}

/// The models each agent will accept.
///
/// Checked against the tools themselves rather than remembered:
///
/// * **Claude Code** documents aliases for its `--model` flag — `claude
///   --help` and the model-configuration page — and has no way to list what
///   an account can actually use. So these are the documented aliases, and
///   the free-text field beside them takes a full name such as
///   `claude-fable-5`.
/// * **Codex** keeps the list its own client fetched in
///   `~/.codex/models_cache.json`. Reading that is better than any list otag
///   could hard-code, because it is the list Codex itself is working from.
enum AgentModels {
    /// Where Codex keeps the models it knows about.
    static func codexCache(home: URL) -> URL {
        home.appendingPathComponent(".codex/models_cache.json")
    }

    /// The aliases Claude Code documents for `--model`.
    ///
    /// In the order a person chooses between them: the two that mean "pick
    /// for me", then the families from most capable down.
    static let claude = [
        "default", "best", "opus", "fable", "sonnet", "haiku", "opusplan",
    ]

    /// What to offer for this agent.
    static func of(_ agent: AgentKind?) -> [String] {
        switch agent {
        case .claude: return claude
        case .codex: return codex()
        case nil: return []
        }
    }

    /// The models Codex's own cache names, newest first.
    ///
    /// Empty when the file is not there or cannot be read, which leaves the
    /// free-text field as the way in — better than offering a list of names
    /// that may have been retired.
    static func codex(home: URL = URL(fileURLWithPath: NSHomeDirectory())) -> [String] {
        guard let data = try? Data(contentsOf: codexCache(home: home)),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let models = json["models"] as? [[String: Any]]
        else { return [] }

        return models.compactMap { model in
            guard let slug = model["slug"] as? String, !slug.isEmpty else { return nil }
            // Codex lists a model it uses for its own review pass, which is
            // not something to put a session on.
            return slug.contains("auto-review") ? nil : slug
        }
    }
}
