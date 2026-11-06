//! Team process orchestration; the session owns durable workflow data.

use otag_core::proto::Response;
use otag_core::session::Session;
use otag_core::team::{Member, Role, Team};
use std::path::Path;
use std::sync::Arc;

/// A waiting role has no interpreter and cannot execute terminal input.
pub fn waiting_command() -> Vec<String> {
    vec!["/bin/sleep".into(), "2147483647".into()]
}

/// Create all role panes before activating the first one.
pub async fn create(session: &Arc<Session>, project: &Path, roles: Vec<Role>) -> Response {
    if let Err(message) = otag_core::team::validate(&roles) {
        return Response::error(message);
    }
    let project = match project.canonicalize() {
        Ok(path) if path.is_dir() => path,
        _ => return Response::error("Choose an existing project folder."),
    };
    let mut members = Vec::new();
    for (index, mut role) in roles.into_iter().enumerate() {
        role.name = role.name.trim().to_owned();
        role.can_edit = role.editable();
        let cwd = if role.worktree {
            match crate::git::add_worktree(&project, &format!("team-{}-{index}", stamp())).await {
                Ok(path) => path,
                Err(message) => return Response::error(message),
            }
        } else {
            project.clone()
        };
        match session
            .start_session(Some(&cwd), &waiting_command(), &role.name)
            .await
        {
            Ok(pane) => {
                session.set_nickname(pane, &role.name).await;
                members.push(Member { role, pane, cwd });
            }
            Err(_) => {
                return Response::error(
                    "Could not create a team session. Check that the project is accessible.",
                );
            }
        }
    }
    let Some(first) = members.first() else {
        return Response::error("Add team roles first.");
    };
    let pane = first.pane;
    let team = Team {
        id: pane.get(),
        project: project.clone(),
        members,
        current: 0,
        automatic: false,
        running: false,
        finished: false,
    };
    let mut teams = session.teams();
    teams.push(team);
    session.set_teams(teams).await;
    if let Err(message) = activate(session, pane.get()).await {
        return Response::error(message);
    }
    Response::SessionStarted {
        pane,
        workspace: project
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        cwd: Some(project),
    }
}

/// Activate one role, retaining stable pane identities for all clients.
pub async fn activate(session: &Arc<Session>, id: u64) -> Result<(), String> {
    let mut teams = session.teams();
    let team = teams
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or("This team no longer exists.")?;
    let member = team
        .members
        .get(team.current)
        .ok_or("This team has no next role.")?;
    let prompt = member.role.instruction(team.current);
    let command = member
        .role
        .agent
        .map(|agent| {
            otag_agents::team::command(agent, &member.cwd, &prompt, !member.role.editable())
        })
        .unwrap_or_default();
    session
        .restart_role(member.pane, &command, &member.role.name)
        .await
        .map_err(|_| {
            "Could not start this role. Check the agent installation and permissions.".to_owned()
        })?;
    session.set_first_prompt(member.pane, &prompt).await;
    team.running = true;
    session.set_teams(teams).await;
    Ok(())
}

fn stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}
