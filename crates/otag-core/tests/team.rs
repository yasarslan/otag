use otag_core::pane::{PaneId, PaneSpec};
use otag_core::session::Session;
use otag_core::team::Team;

#[tokio::test]
async fn read_only_edit_events_are_visible_and_disable_automatic_progression() {
    let session = Session::new(PaneSpec::shell());
    let team: Team = serde_json::from_value(serde_json::json!({
        "id":1,"project":"/project","automatic":true,"members":[
            {"role":{"name":"Review","agent":"codex","can_edit":false},"pane":1,"cwd":"/project"},
            {"role":{"name":"Coder","agent":"codex","can_edit":true},"pane":2,"cwd":"/project"}
        ]
    }))
    .expect("team");
    session.set_teams(vec![team]).await;
    session.note_read_only_edit(PaneId::from_number(2)).await;
    assert!(session.teams()[0].automatic, "an editor is allowed to edit");
    session.note_read_only_edit(PaneId::FIRST).await;
    let team = &session.teams()[0];
    assert!(!team.automatic);
    assert!(
        team.membership(PaneId::FIRST)
            .expect("membership")
            .violation
    );
}

#[tokio::test]
async fn provider_model_preferences_restore_even_without_sessions() {
    let session = Session::new(PaneSpec::shell());
    session
        .set_agent_model(
            otag_agents::AgentKind::Deepseek,
            otag_agents::provider::DEFAULT_MODEL.into(),
        )
        .await;
    let restored = Session::new(PaneSpec::shell());
    restored
        .restore(&session.snapshot(), &|_| Vec::new())
        .await
        .expect("restore");
    assert_eq!(
        restored
            .agent_model(otag_agents::AgentKind::Deepseek)
            .as_deref(),
        Some(otag_agents::provider::DEFAULT_MODEL)
    );
    assert!(restored.is_empty());
}
