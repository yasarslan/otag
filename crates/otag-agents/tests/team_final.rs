use otag_agents::{AgentKind, TurnState};
use std::path::Path;

#[test]
fn completed_turns_supply_final_answers_instead_of_commentary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    for (agent, file) in [
        (AgentKind::Claude, "claude-team-final.jsonl"),
        (AgentKind::Codex, "codex-team-final.jsonl"),
    ] {
        let transcript = agent
            .adapter()
            .read_transcript(&root.join(file))
            .expect("fixture");
        assert!(
            transcript
                .final_message
                .as_deref()
                .is_some_and(|m| m.ends_with("OTAG_RESULT: READY"))
        );
        assert!(
            !transcript
                .final_message
                .as_deref()
                .unwrap_or_default()
                .contains("commentary")
        );
    }
}

#[test]
fn interrupted_or_missing_final_answers_are_not_ready() {
    let transcript = otag_agents::codex::parse_rollout(
        "{\"type\":\"event_msg\",\"payload\":{\"type\":\"turn_aborted\"}}",
        None,
    );
    assert_eq!(transcript.turn, Some(TurnState::Finished));
    assert!(transcript.interrupted);
    assert!(transcript.final_message.is_none());
}

#[test]
fn rollout_tool_events_expose_edit_attempts() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/codex-team-edit.jsonl");
    assert!(
        AgentKind::Codex
            .adapter()
            .read_transcript(&path)
            .expect("fixture")
            .edit_attempt
    );
}
