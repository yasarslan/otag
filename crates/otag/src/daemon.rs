//! The daemon: owns the socket, the panes, and the programs running on them.

use nix::fcntl::{Flock, FlockArg};
use nix::unistd::{Pid, getppid};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use otag_agents::approval::Answer;
use otag_agents::{AgentKind, SessionRef, claude, hooks};
use otag_core::attach;
use otag_core::pane::{PaneId, PaneSize, PaneSpec};
use otag_core::paths::SOCKET_ENV;
use otag_core::proto::{AskView, Event, Handoff as ProtoHandoff, PaneUsage, Request, Response};
use otag_core::raw;
use otag_core::server::{Handler, Outbox, Server};
use otag_core::session::{CloseTarget, Session};
use otag_features::inbox::{AskId, Inbox};

use crate::{agents, events, guard, handoff, inbox, nightshift, notify, persist, usage};

/// Run the daemon in the foreground until it is asked to stop.
pub fn run(
    socket: &Path,
    state: &Path,
    parent: Option<u32>,
    log_filter: Option<&str>,
) -> Result<()> {
    // The lock precedes logs, socket binding and restore. Never unlink it:
    // replacing a locked inode would let a second daemon open the database.
    std::fs::create_dir_all(state).context("cannot create the daemon state directory")?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(state.join("daemon.lock"))
        .context("cannot open the daemon state lock")?;
    let _state_lock = Flock::lock(file, FlockArg::LockExclusiveNonblock)
        .map_err(|(_, error)| anyhow::anyhow!("another otag daemon owns state directory {} ({error}); stop it before starting another", state.display()))?;
    std::fs::write(state.join("daemon.pid"), std::process::id().to_string())?;
    let parent = parent.map(|pid| {
        if pid == 0 {
            getppid().as_raw()
        } else {
            pid as i32
        }
    });
    // Keep the guard alive for the whole run so buffered log lines are
    // flushed before the process exits.
    let log_dir = state.join("logs");
    let _log_guard = match std::fs::create_dir_all(&log_dir) {
        Ok(()) => Some(crate::logging::init_daemon(&log_dir, log_filter)?),
        Err(error) => {
            // Without a log directory the daemon still runs; it just logs to
            // stderr, which `otag` redirects to a file when it spawns us.
            crate::logging::init_client(log_filter.or(Some("info")));
            tracing::warn!(%error, "logging to stderr instead of a file");
            None
        }
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;

    runtime.block_on(serve(socket, state, parent))
}

async fn serve(socket: &Path, state: &Path, parent: Option<i32>) -> Result<()> {
    let server = Server::bind(socket)
        .await
        .with_context(|| format!("cannot start the otag daemon on {}", socket.display()))?;

    tracing::info!(version = otag_core::VERSION, "otag daemon started");
    // Every pane runs the user's shell, told how to reach otag so that agent
    // hooks inside it can report back.
    let session = Session::new(PaneSpec::shell().with_env(SOCKET_ENV, socket));

    // Persistence is a convenience, not a requirement: a daemon that cannot
    // save its state still runs the user's panes.
    let snapshots = open_snapshots(state);
    let saving = match &snapshots {
        Some(snapshots) => {
            persist::restore(&session, snapshots).await;
            Some(tokio::spawn(persist::keep_saving(
                Arc::clone(&session),
                snapshots.clone(),
            )))
        }
        None => None,
    };

    // The usage history outlives one daemon run: a projection needs the last
    // twenty minutes of samples, which a restart would otherwise throw away.
    let shared_usage = usage::Shared::new();
    if let Some(snapshots) = &snapshots {
        persist::restore_usage(&shared_usage, snapshots).await;
    }
    usage::publish(&session, &shared_usage).await;

    // Agents are noticed whether or not the user installed otag's hooks, so
    // this runs for every daemon.
    let watching = tokio::spawn(agents::keep_watching(
        Arc::clone(&session),
        Arc::clone(&shared_usage),
    ));

    // Reaching the user when the terminal is not in front of them is the
    // reason otag exists; it is also entirely optional, and silent when the
    // system offers no way to do it.
    let bus = events::Bus::new();
    let notifying = tokio::spawn({
        let bus = bus.clone();
        notify::keep_watching(Arc::clone(&session), move || bus.has_subscribers())
    });

    // The inbox follows the panes whether or not the agents have hooks, so
    // it runs for every daemon too.
    let inbox = Arc::new(std::sync::Mutex::new(Inbox::new()));
    let collecting = tokio::spawn(inbox::keep_in_step(
        Arc::clone(&session),
        Arc::clone(&inbox),
        bus.clone(),
    ));

    // The night shift only does anything for sessions a client has given a
    // plan, so it runs for every daemon and costs nothing until then.
    let shifts = nightshift::Shifts::new();
    let working = tokio::spawn(nightshift::keep_working(
        Arc::clone(&session),
        Arc::clone(&shifts),
        bus.clone(),
    ));

    // The limit guard is the same shape: nothing happens until a client
    // asks for a session to be watched, and it keeps working with every
    // client closed, which is what it is for.
    let handoffs = Arc::new(std::sync::Mutex::new(handoff::Recent::new()));
    let guards = guard::Guards::new();
    let guarding = tokio::spawn(guard::keep_watching(
        Arc::clone(&session),
        Arc::clone(&guards),
        bus.clone(),
        Arc::clone(&handoffs),
    ));

    let daemon = Daemon {
        session: Arc::clone(&session),
        snapshots: snapshots.clone(),
        shifts: Arc::clone(&shifts),
        guards: Arc::clone(&guards),
        usage: Arc::clone(&shared_usage),
        events: bus,
        attachments: raw::Attachments::new(),
        inbox,
        handoffs,
    };
    let teaming = tokio::spawn(crate::team::keep_working(
        Arc::clone(&session),
        Arc::clone(&guards),
    ));
    let socket_identity = std::fs::metadata(socket).context("cannot inspect the bound socket")?;
    let result = server
        .run_until(daemon, async {
            tokio::select! {
                () = terminate_signal() => {},
                () = lifetime(socket, &socket_identity, parent) => {},
            }
        })
        .await;
    guarding.abort();
    teaming.abort();
    watching.abort();
    notifying.abort();
    collecting.abort();
    working.abort();

    // The panes are about to be killed with the daemon, so what they show
    // now is what a restart has to put back.
    if let Some(snapshots) = &snapshots {
        if let Some(saving) = saving {
            saving.abort();
        }
        persist::save_everything(&session, snapshots).await;
        persist::save_usage(&shared_usage, snapshots).await;
    }

    let daemon_pid = std::process::id() as i32;
    match tokio::task::spawn_blocking(move || otag_core::process::terminate_children(daemon_pid))
        .await
    {
        Ok(Ok(())) => {}
        other => tracing::error!(?other, "cannot finish stopping the pane process trees"),
    }
    tracing::info!("otag daemon stopped");
    result.context("the otag daemon stopped with an error")
}

/// Open the snapshot database, or report why the daemon will not remember
/// anything.
fn open_snapshots(state: &Path) -> Option<persist::Snapshots> {
    match persist::Snapshots::open(state.join(otag_persist::DATABASE_FILE_NAME)) {
        Ok(snapshots) => {
            tracing::info!(database = %snapshots.path().display(), "otag is saving its state");
            Some(snapshots)
        }
        Err(error) => {
            tracing::warn!(%error, "otag will not remember this session");
            None
        }
    }
}

/// A forgotten test socket or a dead launching process is a shutdown request.
async fn lifetime(socket: &Path, identity: &std::fs::Metadata, parent: Option<i32>) {
    let parent_identity = parent.and_then(|pid| {
        otag_core::process::table()
            .ok()?
            .into_iter()
            .find(|p| p.pid == pid)
    });
    loop {
        let exists = std::fs::metadata(socket)
            .is_ok_and(|now| now.dev() == identity.dev() && now.ino() == identity.ino());
        if !exists {
            return;
        }
        if let Some(pid) = parent {
            if pid <= 1 || nix::sys::signal::kill(Pid::from_raw(pid), None).is_err() {
                return;
            }
            if let Some(identity) = &parent_identity {
                match otag_core::process::table() {
                    Ok(all) if !all.iter().any(|p| identity.same(p)) => return,
                    Err(error) => {
                        tracing::warn!(%error, "cannot verify the daemon parent");
                        return;
                    }
                    _ => {}
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Everything one daemon owns.
struct Daemon {
    session: Arc<Session>,
    snapshots: Option<persist::Snapshots>,
    usage: Arc<usage::Shared>,
    /// Carries events to the clients that asked to be told about them.
    events: events::Bus,
    /// The panes clients are watching as streams of bytes.
    attachments: raw::Attachments,
    /// What the agents are waiting for the user to answer.
    inbox: Arc<std::sync::Mutex<Inbox>>,
    /// What otag is working through for the sessions that asked.
    shifts: Arc<nightshift::Shifts>,
    /// The sessions whose limits otag is watching.
    guards: Arc<guard::Guards>,
    /// The panes a handoff has just been made from, so one asked for twice
    /// does not open two sessions.
    handoffs: Arc<std::sync::Mutex<handoff::Recent>>,
}

impl Handler for Daemon {
    /// Answer a client's request.
    ///
    /// `Shutdown` and `Detach` never reach here: the server handles its own
    /// lifecycle. Nothing in here may fail in a way that stops the daemon:
    /// a request that cannot be carried out is logged and answered, and the
    /// session keeps running.
    async fn handle(&self, request: Request, outbox: &Outbox) -> Option<Response> {
        match request {
            Request::Ping => Some(Response::Pong),
            Request::Version => Some(Response::Version {
                version: otag_core::VERSION.to_owned(),
            }),
            Request::ListPanes => Some(Response::Panes {
                panes: self.panes(),
            }),
            Request::SetNickname { pane, nickname } => {
                self.session.set_nickname(pane, &nickname).await;
                Some(Response::Ok)
            }
            Request::TerminalAppearance { dark } => {
                self.session.set_terminal_appearance(dark);
                tracing::debug!(dark, "a client said which way round its terminal is");
                Some(Response::Ok)
            }
            Request::Usage => Some(Response::Usage {
                report: Box::new(self.usage.report()),
                panes: self
                    .session
                    .pane_usage()
                    .into_iter()
                    .map(|(pane, agent, tokens)| PaneUsage {
                        pane,
                        agent,
                        tokens,
                    })
                    .collect(),
            }),
            Request::Attach { size, cwd } => Some(self.attach(outbox, size, cwd.as_deref()).await),
            Request::Input { bytes } => {
                if let Err(error) = self.session.input(bytes.into_vec()).await {
                    // A key pressed as a program exits has nowhere to go.
                    // The client learns the pane ended from its own stream.
                    tracing::debug!(%error, "cannot deliver input to a pane");
                }
                None
            }
            Request::Resize { size } => {
                self.session.resize(size).await;
                None
            }
            Request::Split { direction } => {
                self.report("split a pane", self.session.split(direction).await)
            }
            Request::NewTab => self.report("add a tab", self.session.new_tab().await),
            Request::SelectTab { index } => {
                self.session.select_tab(index).await;
                None
            }
            Request::NewWorkspace { cwd } => self.report(
                "add a workspace",
                self.session.new_workspace(cwd.as_deref()).await,
            ),
            Request::SelectWorkspace { index } => {
                self.session.select_workspace(index).await;
                None
            }
            Request::FocusPane { pane } => {
                self.session.focus_pane(pane).await;
                None
            }
            Request::FocusSide { side } => {
                self.session.focus_side(side).await;
                None
            }
            Request::FocusNext => {
                self.session.focus_next().await;
                None
            }
            Request::ResizePane { side, cells } => {
                self.session.resize_pane(side, cells).await;
                None
            }
            Request::ToggleSidebar => {
                self.session.toggle_sidebar().await;
                None
            }
            Request::StartSession {
                cwd,
                agent,
                model,
                prompt,
                worktree,
            } => Some(
                self.start_session(StartSession {
                    cwd,
                    agent,
                    model,
                    prompt,
                    worktree,
                })
                .await,
            ),
            Request::StartTeam { cwd, roles, task } => {
                Some(crate::team::create(&self.session, &cwd, roles, &task).await)
            }
            Request::TeamAdvance {
                pane,
                revision,
                back,
                confirmation,
            } => {
                Some(crate::team::advance(&self.session, pane, revision, back, confirmation).await)
            }
            Request::TeamMode { team, automatic } => {
                let response = crate::team::mode(&self.session, team, automatic).await;
                if matches!(response, Response::Ok)
                    && let Some(snapshots) = &self.snapshots
                    && !snapshots.save_session(&self.session).await
                {
                    return Some(Response::error(
                        "Could not save the team's Automatic setting.",
                    ));
                }
                Some(response)
            }
            Request::TeamUndoPreview { pane, step } => {
                Some(crate::team::undo_preview(&self.session, pane, step).await)
            }
            Request::TeamUndoConfirm {
                team,
                step,
                expected,
            } => Some(crate::team::undo_confirm(&self.session, team, step, expected).await),
            Request::ApiProvider {
                provider,
                model,
                refresh,
            } => Some(self.api_provider(&provider, model, refresh).await),
            Request::AttachPane { pane, size } => self.attach_pane(outbox, pane, size).await,
            Request::DetachPane { pane } => {
                self.attachments.remove(outbox.id(), pane);
                Some(Response::Ok)
            }
            Request::ClosePane { pane, confirmed } => {
                Some(self.close(CloseTarget::Pane(pane), confirmed).await)
            }
            Request::EndTeam { team, confirmed } => {
                Some(self.close(CloseTarget::Team(team), confirmed).await)
            }
            Request::PaneInput { pane, bytes } => {
                if let Err(error) = self.session.input_to(pane, bytes.into_vec()).await {
                    tracing::debug!(%error, %pane, "cannot deliver input to a pane");
                }
                None
            }
            Request::PaneResize { pane, size } => {
                if let Err(error) = self.session.resize_pane_to(pane, size).await {
                    tracing::debug!(%error, %pane, "cannot resize a pane");
                }
                None
            }
            Request::Subscribe => Some(events::start(&self.session, &self.events, outbox).await),
            Request::NightShift { pane, plan } => {
                if plan.is_some()
                    && self
                        .session
                        .teams()
                        .iter()
                        .any(|t| t.members.iter().any(|m| m.pane == pane))
                {
                    return Some(Response::error(
                        "Team roles use team progression. Turn off the separate task queue for this role.",
                    ));
                }
                if self.session.pane(pane).is_none() {
                    Some(Response::error(format!("otag has no pane {pane}")))
                } else {
                    let stopping = plan.is_none();
                    self.shifts.set(pane, plan);
                    tracing::info!(%pane, stopping, "a night shift plan changed");
                    // The pane list carries the plan, so every client sees
                    // it without asking.
                    self.events.publish(Event::Panes {
                        panes: self.panes(),
                    });
                    Some(Response::Ok)
                }
            }
            Request::Guard { pane, plan } => {
                if self
                    .session
                    .teams()
                    .iter()
                    .any(|t| t.members.iter().any(|m| m.pane == pane))
                    && plan
                        .as_ref()
                        .is_some_and(|p| p.when_reached != otag_core::proto::GuardAction::Wait)
                {
                    return Some(Response::error(
                        "Team roles need a waiting limit guard. Use the team controls to continue or switch roles.",
                    ));
                }
                if self.session.pane(pane).is_none() {
                    Some(Response::error(format!("otag has no pane {pane}")))
                } else {
                    let stopping = plan.is_none();
                    self.guards.set(pane, plan);
                    tracing::info!(%pane, stopping, "a limit guard changed");
                    // The pane list carries the plan, so every client sees
                    // it without asking.
                    self.events.publish(Event::Panes {
                        panes: self.panes(),
                    });
                    Some(Response::Ok)
                }
            }
            Request::MorningReport => Some(Response::MorningReport {
                sessions: nightshift::report(&self.session, &self.shifts).await,
            }),
            Request::UsageHistory => Some(Response::UsageHistory {
                samples: self.usage.samples(),
            }),
            Request::Inbox => Some(Response::Inbox { asks: self.asks() }),
            Request::Answer { ask, answer } => {
                Some(self.answer(AskId::from_number(ask), answer).await)
            }
            Request::Handoff { pane, handoff } => Some(self.handoff(pane, handoff).await),
            Request::Hook {
                event,
                pane,
                payload,
            } => {
                self.hook(&event, pane, &payload).await;
                None
            }
            Request::Shutdown | Request::Detach | Request::Unknown => Some(Response::error(
                "this otag daemon cannot carry out that request yet",
            )),
        }
    }
}

/// What a client asked for when it asked for a new session.
struct StartSession {
    cwd: Option<PathBuf>,
    agent: Option<AgentKind>,
    model: Option<String>,
    prompt: Option<String>,
    worktree: bool,
}

impl Daemon {
    async fn close(&self, target: CloseTarget, confirmed: bool) -> Response {
        match self.session.close_sessions(target, confirmed).await {
            Ok(false) => Response::CloseConfirmationRequired,
            Ok(true) => {
                if let Some(snapshots) = &self.snapshots
                    && !snapshots.save_session(&self.session).await
                {
                    return Response::error(
                        "Closed the sessions, but could not save the change. Check the daemon state directory before restarting.",
                    );
                }
                Response::Ok
            }
            Err(error) => {
                tracing::warn!(%error, "cannot close the requested sessions");
                Response::error(format!("Could not close the sessions: {error}. Try again."))
            }
        }
    }

    async fn api_provider(&self, provider: &str, model: Option<String>, refresh: bool) -> Response {
        use otag_agents::provider as api;
        if provider != AgentKind::Deepseek.name() {
            return Response::error("This API provider is not supported yet.");
        }
        if let Some(model) = model {
            if !api::MODELS.contains(&model.as_str()) {
                return Response::error("Choose a supported model in API LLM.");
            }
            self.session
                .set_agent_model(AgentKind::Deepseek, model)
                .await;
        }
        let configured = tokio::task::spawn_blocking(api::configured)
            .await
            .unwrap_or(false);
        let result = if refresh && configured {
            Some(tokio::task::spawn_blocking(api::balance).await)
        } else {
            None
        };
        let connected = result.as_ref().map(|r| matches!(r, Ok(Ok(_))));
        let balance = result.and_then(|r| r.ok()).and_then(|r| r.ok()).flatten();
        Response::ApiProvider {
            provider: provider.into(),
            model: self
                .session
                .agent_model(AgentKind::Deepseek)
                .unwrap_or_else(|| api::DEFAULT_MODEL.into()),
            configured,
            connected,
            balance,
            models: api::MODELS.iter().map(|s| (*s).into()).collect(),
            keychain_service: api::SERVICE.into(),
            keychain_account: api::ACCOUNT.into(),
        }
    }
    /// Every pane, with what the night shift and the guard are doing for it.
    fn panes(&self) -> Vec<otag_core::proto::PaneInfo> {
        let mut panes = events::panes_of(&self.session);
        let usage = self.usage.report();
        for pane in &mut panes {
            pane.night_shift = self.shifts.view(pane.id);
            // The guard's view carries the usage it is deciding on, so a
            // client shows the same number the guard acted on rather than
            // working one out of its own.
            pane.guard = pane.agent.and_then(|agent| {
                self.guards.view(
                    pane.id,
                    otag_features::nightshift::worst_used(&usage, agent),
                )
            });
        }
        panes
    }

    /// Start a session: a project, an agent, and what to ask it first.
    ///
    /// Answered either way, because the user is waiting on it. A worktree
    /// that cannot be made is reported and the session is *not* started: a
    /// user who asked for one does not want two agents in one checkout.
    async fn start_session(&self, asked: StartSession) -> Response {
        let mut cwd = asked.cwd;
        if asked.worktree {
            let Some(project) = cwd.clone() else {
                return Response::error("a worktree needs a project directory to make it in.");
            };
            let name = format!(
                "{}-{}",
                asked.agent.map_or("shell", AgentKind::name),
                worktree_stamp()
            );
            match crate::git::add_worktree(&project, &name).await {
                Ok(worktree) => cwd = Some(worktree),
                Err(message) => return Response::error(message),
            }
        }

        let configured_model = asked
            .agent
            .and_then(|agent| self.session.agent_model(agent));
        let (command, title) = match asked.agent {
            Some(agent) => (
                agent.adapter().start_command(
                    asked.prompt.as_deref(),
                    asked.model.as_deref().or(configured_model.as_deref()),
                ),
                agent.name().to_owned(),
            ),
            // No agent is a shell, and a prompt without an agent has nothing
            // to be asked of, so it is ignored rather than typed somewhere.
            None => (Vec::new(), "shell".to_owned()),
        };

        match self
            .session
            .start_session(cwd.as_deref(), &command, &title)
            .await
        {
            Ok(pane) => {
                self.session.set_agent(pane, asked.agent).await;
                // What the client asked for is what the session is for, so
                // it is its name from the first moment rather than once the
                // agent has written its transcript.
                if let Some(prompt) = asked.prompt.as_deref() {
                    self.session.set_first_prompt(pane, prompt).await;
                }

                let workspace = self
                    .session
                    .summaries()
                    .into_iter()
                    .find(|summary| summary.id == pane)
                    .map(|summary| summary.workspace)
                    .unwrap_or_default();
                tracing::info!(%pane, workspace, "started a session");
                Response::SessionStarted {
                    pane,
                    workspace,
                    cwd,
                }
            }
            Err(error) => {
                tracing::warn!(%error, "cannot start a session");
                Response::error(format!("cannot start a session: {error}"))
            }
        }
    }

    /// Everything the agents are waiting for, as a client sees it.
    fn asks(&self) -> Vec<AskView> {
        self.inbox
            .lock()
            .map(|inbox| inbox.asks().iter().map(AskView::of).collect())
            .unwrap_or_default()
    }

    /// Watch one pane as a stream of bytes.
    async fn attach_pane(
        &self,
        outbox: &Outbox,
        pane: PaneId,
        size: Option<PaneSize>,
    ) -> Option<Response> {
        match raw::attach_pane(&self.session, &self.attachments, outbox, pane, size).await {
            Ok(()) => None,
            Err(error) => {
                tracing::debug!(%error, %pane, "cannot attach a client to a pane");
                Some(Response::error(error.to_string()))
            }
        }
    }

    /// Answer what an agent is waiting for, from outside its pane.
    ///
    /// Answered either way, and never optimistically: otag types the keys
    /// the agent's own prompt offers and then checks that the prompt is
    /// gone. A question that cannot be answered this way says so, so that
    /// the user goes to the pane instead of believing it was handled.
    async fn answer(&self, ask: AskId, answer: Answer) -> Response {
        let taken = self.inbox.lock().ok().and_then(|mut inbox| inbox.take(ask));
        let Some(waiting) = taken else {
            return Response::Answered {
                ask: ask.get(),
                delivered: false,
                summary: "that question is not waiting any more".to_owned(),
            };
        };

        let pane = PaneId::from_number(waiting.pane);
        let outcome = inbox::deliver(&self.session, pane, waiting.agent, answer).await;
        self.events.publish(Event::Resolved {
            ask: ask.get(),
            answer: Some(answer),
        });

        Response::Answered {
            ask: ask.get(),
            delivered: outcome.delivered,
            summary: outcome.summary,
        }
    }

    /// Carry a pane's work over to another agent, or another model.
    ///
    /// Answered either way: a handoff is something the user asked for and
    /// waits on, so a refusal has to say why.
    async fn handoff(&self, pane: PaneId, handoff: ProtoHandoff) -> Response {
        if self
            .session
            .teams()
            .iter()
            .any(|t| t.members.iter().any(|m| m.pane == pane))
        {
            let current = self
                .session
                .teams()
                .iter()
                .flat_map(|t| &t.members)
                .find(|m| m.pane == pane)
                .and_then(|m| m.role.agent);
            let (target, model) = match handoff {
                ProtoHandoff::Agent { to } => (
                    to.unwrap_or_else(|| {
                        otag_features::guard::other_than(current.unwrap_or(AgentKind::Claude))
                    }),
                    None,
                ),
                ProtoHandoff::Model { model: Some(model) } => {
                    (current.unwrap_or(AgentKind::Claude), Some(model))
                }
                _ => return Response::error("Choose an agent or model for this team role."),
            };
            return match crate::team::switch_agent(&self.session, pane, target, model).await {
                Ok(()) => Response::HandoffDone {
                    pane,
                    package: None,
                    summary: "Team role agent updated.".into(),
                },
                Err(message) => Response::error(message),
            };
        }
        let mode = match handoff {
            ProtoHandoff::Agent { to } => handoff::Mode::SwitchAgent { to },
            ProtoHandoff::Model { model } => handoff::Mode::SwitchModel { model },
            ProtoHandoff::Unknown => {
                return Response::error(
                    "this otag daemon does not know that kind of handoff.                      The daemon and the client are probably different versions;                      restart the daemon with `otag kill-server`.",
                );
            }
        };

        // A handoff opens a session and writes a file, so the second click
        // on the button has to be refused rather than obeyed. A guard that
        // cannot be read does not block the user: the worst that can then
        // happen is the thing this prevents.
        if let Ok(mut recent) = self.handoffs.lock()
            && let Err(message) = recent.claim(pane)
        {
            tracing::info!(%pane, message, "a repeated handoff was refused");
            return Response::error(message);
        }

        match handoff::run(&self.session, pane, mode).await {
            Ok(done) => {
                // Every client learns about it, not only the one that asked:
                // a handoff changes what the window should be showing.
                self.events.publish(Event::Handoff {
                    pane: done.pane,
                    package: done.package.clone(),
                    summary: done.summary.clone(),
                });
                Response::HandoffDone {
                    pane: done.pane,
                    package: done.package,
                    summary: done.summary,
                }
            }
            Err(message) => {
                // Nothing was opened and nothing was written, so the user
                // can fix what was wrong and ask again at once.
                if let Ok(mut recent) = self.handoffs.lock() {
                    recent.release(pane);
                }
                tracing::info!(%pane, message, "a handoff was refused");
                Response::error(message)
            }
        }
    }

    /// Act on what an agent's hook, or its status line, reported.
    ///
    /// A hook is the agent speaking for itself, so what it says outweighs
    /// any guess from its screen. Nothing here is trusted blindly: a payload
    /// that cannot be read, or one for a pane this daemon does not have, is
    /// logged at debug level and dropped.
    async fn hook(&self, event: &str, pane: Option<PaneId>, payload: &str) {
        // A pane otag is not running is the same as no pane at all: there is
        // nothing to attribute to it.
        let pane = pane.filter(|pane| self.session.pane(*pane).is_some());

        if event == hooks::STATUS_LINE {
            // The one event that is worth having without a pane. What it
            // carries about the account's limits is true of the whole
            // machine, whoever ran the session: a Claude Code session in
            // another terminal spends the same five hours otag is drawing.
            self.status_line(pane, payload).await;
            return;
        }

        let Some(pane) = pane else {
            // Everything else describes one session's work, and otag has
            // nowhere to put it.
            tracing::debug!(event, "a hook arrived without a pane otag is running");
            return;
        };

        let Some(hook) = hooks::HookPayload::parse(payload) else {
            tracing::debug!(event, %pane, "a hook payload could not be read");
            return;
        };
        if hook.written_file(event).is_some() {
            self.session.note_read_only_edit(pane).await;
        }
        let team_role = self.session.is_team_role(pane);
        let status = if team_role {
            hook.team_status(event)
        } else {
            hook.status(event)
        };
        if let Some(status) = status {
            self.session.set_status(pane, status, true).await;
        }
        if status.is_some() || !team_role {
            self.collect_question(status, pane, &hook);
        }
        if let Some(id) = hook.session_id.clone() {
            self.remember_session(pane, id, hook.transcript_path, hook.cwd)
                .await;
        }
    }

    /// Put what an agent is asking into the inbox, or take it out again
    /// because the agent has moved on.
    fn collect_question(
        &self,
        status: Option<otag_agents::PaneStatus>,
        pane: PaneId,
        hook: &hooks::HookPayload,
    ) {
        let asking = status == Some(otag_agents::PaneStatus::NeedsApproval);
        let Ok(mut inbox) = self.inbox.lock() else {
            return;
        };

        if asking {
            let agent = self
                .session
                .agent_panes()
                .into_iter()
                .find(|known| known.id == pane)
                .and_then(|known| known.agent);
            let summary = hook.summary().unwrap_or_else(|| "needs you".to_owned());
            let id = inbox.ask(pane.get(), agent, hook.tool_name.clone(), summary);
            if let Some(ask) = inbox.get(id) {
                self.events.publish(Event::Asked {
                    ask: AskView::of(ask),
                });
            }
            return;
        }

        // Anything else from the same pane means it is not waiting any
        // more: the user answered in the pane, or the agent gave up asking.
        let Some(waiting) = inbox.of_pane(pane.get()).map(|ask| ask.id) else {
            return;
        };
        inbox.clear_pane(pane.get());
        self.events.publish(Event::Resolved {
            ask: waiting.get(),
            answer: None,
        });
    }

    /// Act on what the status line otag registered reported.
    ///
    /// It says nothing about what the agent is doing, but it is the one
    /// place the session identifier and the account's usage arrive from
    /// without otag having to read the agent's files.
    ///
    /// `pane` is the session's pane when otag is running it. Without one the
    /// usage is still kept — it belongs to the account, not to the pane —
    /// and only the per-session part is skipped.
    async fn status_line(&self, pane: Option<PaneId>, payload: &str) {
        let Some(line) = claude::StatusLine::parse(payload) else {
            tracing::debug!(?pane, "a status line payload could not be read");
            return;
        };
        if let (Some(pane), Some(id)) = (pane, line.session_id.clone()) {
            self.remember_session(pane, id, line.transcript_path.clone(), line.cwd.clone())
                .await;
        }

        // The status line is the only place Claude Code says how much of the
        // account's limits is gone. Everything else otag shows about usage is
        // counted, and says so.
        let source = pane
            .and_then(|id| {
                self.session
                    .agent_panes()
                    .into_iter()
                    .find(|p| p.id == id)
                    .and_then(|p| p.agent)
            })
            .unwrap_or(AgentKind::Claude);
        if self
            .usage
            .record_limits(source, &line.limits(), line.session_id.as_deref())
        {
            usage::publish(&self.session, &self.usage).await;
        }
    }

    /// Record which session a pane is running, and how to continue it.
    async fn remember_session(
        &self,
        pane: PaneId,
        id: String,
        transcript: Option<PathBuf>,
        cwd: Option<PathBuf>,
    ) {
        if let Some(path) = transcript.clone() {
            self.session.set_session_path(pane, path).await;
        }
        if !self.session.set_session_id(pane, id.clone()).await {
            // Already known, so the restore command is already right.
            return;
        }

        // Only Claude Code has hooks, so this is the agent that sent them.
        let reference = SessionRef {
            id,
            path: transcript.unwrap_or_default(),
            cwd,
        };
        let agent = self
            .session
            .agent_panes()
            .into_iter()
            .find(|p| p.id == pane)
            .and_then(|p| p.agent)
            .unwrap_or(AgentKind::Claude);
        let resume = agent.adapter().resume_command(&reference);
        self.session.set_restore_command(pane, resume).await;
    }

    /// Attach a client, starting a workspace and a pane if there is none.
    async fn attach(&self, outbox: &Outbox, size: PaneSize, cwd: Option<&Path>) -> Response {
        match attach::start(&self.session, outbox, size, cwd).await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(%error, "cannot attach a client to the session");
                Response::error(error.to_string())
            }
        }
    }

    /// Turn a failed structural change into an answer the user can act on.
    ///
    /// A change that worked needs no answer: every attached client is sent
    /// the new view anyway.
    fn report(&self, what: &str, result: otag_core::Result<()>) -> Option<Response> {
        match result {
            Ok(()) => None,
            Err(error) => {
                tracing::warn!(%error, what, "cannot carry out a request");
                Some(Response::error(format!("cannot {what}: {error}")))
            }
        }
    }
}

/// A short, sortable stamp for naming a worktree and its branch.
///
/// Seconds since the epoch: short, unique enough for a person starting
/// sessions by hand, and it sorts the way the directory listing should.
fn worktree_stamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs().to_string())
        .unwrap_or_else(|_| "new".to_owned())
}

/// Completes when the operating system asks the daemon to terminate.
///
/// A signal handler that cannot be installed is logged and then ignored: the
/// daemon keeps serving and can still be stopped with `otag kill-server`.
async fn terminate_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut interrupt = match signal(SignalKind::interrupt()) {
        Ok(signal) => signal,
        Err(error) => {
            tracing::warn!(%error, "cannot listen for SIGINT");
            return std::future::pending().await;
        }
    };
    let mut terminate = match signal(SignalKind::terminate()) {
        Ok(signal) => signal,
        Err(error) => {
            tracing::warn!(%error, "cannot listen for SIGTERM");
            interrupt.recv().await;
            return;
        }
    };

    tokio::select! {
        _ = interrupt.recv() => tracing::info!("SIGINT received"),
        _ = terminate.recv() => tracing::info!("SIGTERM received"),
    }
}

#[cfg(test)]
mod tests {
    use otag_agents::PaneStatus;
    use otag_core::pane::PaneSize;

    use super::*;

    /// A daemon with one pane, which is what a hook needs to report against.
    async fn with_one_pane() -> (Daemon, PaneId) {
        let session = Session::new(PaneSpec::command(["sleep", "30"]));
        session
            .attach(PaneSize::new(24, 80), None)
            .await
            .expect("a pane");
        let pane = session
            .visible_panes()
            .first()
            .copied()
            .expect("the pane is there");
        (
            Daemon {
                session,
                snapshots: None,
                usage: usage::Shared::new(),
                events: events::Bus::new(),
                attachments: raw::Attachments::new(),
                inbox: Arc::new(std::sync::Mutex::new(Inbox::new())),
                shifts: nightshift::Shifts::new(),
                guards: guard::Guards::new(),
                handoffs: Arc::new(std::sync::Mutex::new(handoff::Recent::new())),
            },
            pane,
        )
    }

    #[tokio::test]
    async fn starting_a_session_without_an_agent_opens_a_shell_in_the_project() {
        let (daemon, _) = with_one_pane().await;
        let directory = tempfile::tempdir().expect("a temporary directory");

        let response = daemon
            .start_session(StartSession {
                cwd: Some(directory.path().to_path_buf()),
                agent: None,
                model: None,
                prompt: None,
                worktree: false,
            })
            .await;

        let Response::SessionStarted {
            pane,
            workspace,
            cwd,
        } = response
        else {
            panic!("expected a started session, got {response:?}");
        };
        assert!(daemon.session.pane(pane).is_some(), "the pane is running");
        assert!(
            !workspace.is_empty(),
            "the sidebar has something to call it"
        );
        assert_eq!(cwd.as_deref(), Some(directory.path()));
    }

    #[tokio::test]
    async fn asking_for_a_worktree_without_a_project_is_refused_rather_than_guessed_at() {
        let (daemon, _) = with_one_pane().await;

        let response = daemon
            .start_session(StartSession {
                cwd: None,
                agent: None,
                model: None,
                prompt: None,
                worktree: true,
            })
            .await;

        let Response::Error { message } = response else {
            panic!("expected a refusal, got {response:?}");
        };
        assert!(message.contains("project directory"), "{message}");
    }

    #[tokio::test]
    async fn a_worktree_that_cannot_be_made_stops_the_session_rather_than_sharing_a_checkout() {
        // Not a git repository, so there is no worktree to be had — and two
        // agents in one checkout is exactly what the user asked to avoid.
        let (daemon, _) = with_one_pane().await;
        let directory = tempfile::tempdir().expect("a temporary directory");

        let response = daemon
            .start_session(StartSession {
                cwd: Some(directory.path().to_path_buf()),
                agent: None,
                model: None,
                prompt: None,
                worktree: true,
            })
            .await;

        let Response::Error { message } = response else {
            panic!("expected a refusal, got {response:?}");
        };
        assert!(message.contains("worktree"), "{message}");
    }

    #[tokio::test]
    async fn a_hook_says_what_the_agent_is_doing() {
        let (daemon, pane) = with_one_pane().await;

        daemon
            .hook("Notification", Some(pane), r#"{"message":"Needs your ok"}"#)
            .await;

        let panes = daemon.session.agent_panes();
        let found = panes.first().expect("the pane");
        assert_eq!(found.status, PaneStatus::NeedsApproval);
        assert!(
            !found.trusts_the_screen,
            "an agent reporting for itself outranks its screen"
        );
    }

    #[tokio::test]
    async fn a_question_on_the_screen_is_heard_even_just_after_the_agent_reported() {
        let (daemon, pane) = with_one_pane().await;

        daemon.hook("Stop", Some(pane), "{}").await;
        assert!(
            !daemon
                .session
                .agent_panes()
                .first()
                .expect("the pane")
                .trusts_the_screen
        );

        // Everything else the screen could say is ignored while the agent is
        // reporting for itself, but a question is not: an agent without
        // hooks has no other way to say it, and a missed one strands the
        // session.
        daemon
            .session
            .set_status(pane, PaneStatus::Working, false)
            .await;
        assert_eq!(
            daemon.session.agent_panes().first().map(|pane| pane.status),
            Some(PaneStatus::Done)
        );

        daemon
            .session
            .set_status(pane, PaneStatus::NeedsApproval, false)
            .await;
        assert_eq!(
            daemon.session.agent_panes().first().map(|pane| pane.status),
            Some(PaneStatus::NeedsApproval)
        );
    }

    #[tokio::test]
    async fn a_hook_that_carries_a_session_makes_the_pane_resume_it() {
        let (daemon, pane) = with_one_pane().await;

        daemon
            .hook(
                "SessionStart",
                Some(pane),
                r#"{"session_id":"abc-123","source":"startup"}"#,
            )
            .await;

        let summary = daemon
            .session
            .summaries()
            .into_iter()
            .next()
            .expect("the pane");
        assert_eq!(summary.session.as_deref(), Some("abc-123"));
        assert!(
            daemon
                .session
                .snapshot()
                .panes
                .iter()
                .any(|pane| pane.command == ["claude", "--resume", "abc-123"]),
            "the pane comes back by continuing the session"
        );
    }

    #[tokio::test]
    async fn the_status_line_carries_the_session_too() {
        let (daemon, pane) = with_one_pane().await;

        daemon
            .hook(
                otag_agents::hooks::STATUS_LINE,
                Some(pane),
                r#"{"session_id":"from-status-line","model":{"id":"x"}}"#,
            )
            .await;

        let summary = daemon
            .session
            .summaries()
            .into_iter()
            .next()
            .expect("the pane");
        assert_eq!(summary.session.as_deref(), Some("from-status-line"));
    }

    #[tokio::test]
    async fn a_hook_for_an_unknown_pane_changes_nothing() {
        let (daemon, pane) = with_one_pane().await;
        let unknown = pane.next().next();

        daemon.hook("Stop", Some(unknown), "{}").await;
        daemon.hook("Stop", None, "{}").await;

        let panes = daemon.session.agent_panes();
        assert_eq!(
            panes.first().map(|pane| pane.status),
            Some(PaneStatus::Idle)
        );
    }

    #[tokio::test]
    async fn a_payload_that_is_not_json_is_dropped_rather_than_believed() {
        let (daemon, pane) = with_one_pane().await;

        daemon
            .hook("Stop", Some(pane), "<html>not json</html>")
            .await;
        daemon
            .hook(otag_agents::hooks::STATUS_LINE, Some(pane), "")
            .await;

        let panes = daemon.session.agent_panes();
        let found = panes.first().expect("the pane");
        assert_eq!(found.status, PaneStatus::Idle);
        assert_eq!(found.session_id, None);
    }

    /// Where the redacted samples of real agent files live.
    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures");

    #[tokio::test]
    async fn the_status_line_of_a_real_session_becomes_the_usage_report() {
        let (daemon, pane) = with_one_pane().await;
        let payload =
            std::fs::read_to_string(std::path::Path::new(FIXTURES).join("claude-statusline.json"))
                .expect("the fixture");

        daemon
            .hook(otag_agents::hooks::STATUS_LINE, Some(pane), &payload)
            .await;

        let report = daemon.usage.report();
        assert_eq!(report.rows.len(), 2, "both windows the agent reported");
        assert_eq!(
            report.rows[0].window,
            otag_features::usage::Window::FiveHour
        );
        assert_eq!(
            report.rows[0].used_pct(),
            Some(70),
            "what the widget shows is what the agent said"
        );
        assert_eq!(
            report.rows[1].used_pct(),
            Some(58),
            "the weekly window, rounded from the 57.99 the agent sent"
        );
        assert_eq!(
            report.level(),
            otag_features::usage::Level::Amber,
            "seventy per cent is worth knowing about"
        );

        // The clients are told without asking.
        assert_eq!(daemon.session.view().usage, report);
    }

    #[tokio::test]
    async fn a_status_line_from_a_session_outside_otag_still_counts_against_the_account() {
        let (daemon, _pane) = with_one_pane().await;
        let payload =
            std::fs::read_to_string(std::path::Path::new(FIXTURES).join("claude-statusline.json"))
                .expect("the fixture");

        // No pane: Claude Code running in the user's own terminal, where
        // otag's environment was never inherited. The limits it reports are
        // the same account's.
        daemon
            .hook(otag_agents::hooks::STATUS_LINE, None, &payload)
            .await;

        let report = daemon.usage.report();
        assert_eq!(report.rows.len(), 2, "the account's windows are known");
        assert_eq!(report.rows[0].used_pct(), Some(70));
        assert_eq!(
            report.rows[1].used_pct(),
            Some(58),
            "both windows, from a session otag is not running"
        );
        assert_eq!(daemon.session.view().usage, report, "the clients are told");
    }

    #[tokio::test]
    async fn a_status_line_for_a_pane_the_daemon_does_not_have_still_counts() {
        let (daemon, _pane) = with_one_pane().await;
        let payload =
            std::fs::read_to_string(std::path::Path::new(FIXTURES).join("claude-statusline.json"))
                .expect("the fixture");

        daemon
            .hook(
                otag_agents::hooks::STATUS_LINE,
                Some(otag_core::pane::PaneId::from_number(9_999)),
                &payload,
            )
            .await;

        assert_eq!(
            daemon.usage.report().rows.len(),
            2,
            "a pane otag never had is no reason to throw the account's numbers away"
        );
    }

    #[tokio::test]
    async fn a_hook_that_is_not_the_status_line_needs_a_pane() {
        let (daemon, _pane) = with_one_pane().await;

        // This one describes one session's work, so without a pane there is
        // nowhere for it to go.
        daemon
            .hook("Notification", None, r#"{"session_id":"outside"}"#)
            .await;

        let panes = daemon.session.agent_panes();
        assert_eq!(
            panes.first().and_then(|pane| pane.session_id.clone()),
            None,
            "otag's own pane is not given another session's identity"
        );
    }

    #[tokio::test]
    async fn a_status_line_from_an_account_without_limits_adds_no_numbers() {
        let (daemon, pane) = with_one_pane().await;
        let payload = std::fs::read_to_string(
            std::path::Path::new(FIXTURES).join("claude-statusline-no-usage.json"),
        )
        .expect("the fixture");

        daemon
            .hook(otag_agents::hooks::STATUS_LINE, Some(pane), &payload)
            .await;

        assert!(
            daemon.usage.report().is_empty(),
            "an account with no limits to report is not shown as zero per cent"
        );
    }
}
