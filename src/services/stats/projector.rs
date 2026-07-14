use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use crate::{
    infra::{AnonymousUserClaims, UserClaims},
    models::{
        game::MatchEvent,
        id::{MatchId, PlayerId},
    },
    services::{
        repositories::{matches::MatchesRepository, stats::StatsRepository},
        stats::project_match_stats,
    },
};

#[derive(Clone)]
pub struct StatsProjectorHandle {
    tx: mpsc::UnboundedSender<StatsProjectorMessage>,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
    closed: Arc<AtomicBool>,
}

impl StatsProjectorHandle {
    pub(crate) fn notify_match_finished(&self, match_id: &MatchId) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }

        if let Err(e) = self
            .tx
            .send(StatsProjectorMessage::Project(match_id.clone()))
        {
            tracing::error!("Error enqueueing stats projection for {match_id:?}: {e}");
        }
    }

    pub(crate) async fn shutdown(&self, task_timeout: Duration) {
        let should_send_shutdown = !self.closed.swap(true, Ordering::SeqCst);

        if should_send_shutdown {
            let (respond, wait) = oneshot::channel();

            if self
                .tx
                .send(StatsProjectorMessage::Shutdown { respond })
                .is_ok()
            {
                let _ = tokio::time::timeout(task_timeout, wait).await;
            }
        }

        let handle = self
            .task
            .lock()
            .expect("stats projector lock poisoned")
            .take();

        if let Some(mut handle) = handle
            && tokio::time::timeout(task_timeout, &mut handle)
                .await
                .is_err()
        {
            handle.abort();
            let _ = handle.await;
        }
    }

    pub(crate) fn abort(&self) {
        self.closed.store(true, Ordering::SeqCst);

        if let Some(handle) = self
            .task
            .lock()
            .expect("stats projector lock poisoned")
            .take()
        {
            handle.abort();
        }
    }
}

enum StatsProjectorMessage {
    Project(MatchId),
    Shutdown { respond: oneshot::Sender<()> },
}

pub struct StatsProjector;

impl StatsProjector {
    pub fn start(
        matches_repo: MatchesRepository,
        stats_repo: StatsRepository,
    ) -> StatsProjectorHandle {
        let (tx, rx) = mpsc::unbounded_channel();
        let task = StatsProjectorTask {
            matches_repo,
            stats_repo,
            rx,
        };

        let task = tokio::spawn(async move { task.run().await });

        StatsProjectorHandle {
            tx,
            task: Arc::new(Mutex::new(Some(task))),
            closed: Arc::new(AtomicBool::new(false)),
        }
    }
}

struct StatsProjectorTask {
    matches_repo: MatchesRepository,
    stats_repo: StatsRepository,
    rx: mpsc::UnboundedReceiver<StatsProjectorMessage>,
}

impl StatsProjectorTask {
    async fn run(mut self) {
        let matches_repo = self.matches_repo.clone();
        let mut finished_matches = Box::pin(matches_repo.finished_match_ids());
        let mut startup_scan_pending = true;

        loop {
            tokio::select! {
                result = &mut finished_matches, if startup_scan_pending => {
                    startup_scan_pending = false;
                    self.project_finished_matches(result).await;
                }
                message = self.rx.recv() => {
                    let Some(message) = message else { break };
                    if self.handle_message(message).await {
                        break;
                    }
                }
            }
        }
    }

    async fn handle_message(&mut self, message: StatsProjectorMessage) -> bool {
        match message {
            StatsProjectorMessage::Project(match_id) => {
                self.project_match(match_id).await;
                false
            }
            StatsProjectorMessage::Shutdown { respond } => {
                let _ = respond.send(());
                true
            }
        }
    }

    async fn project_finished_matches(&mut self, result: mongodb::error::Result<Vec<MatchId>>) {
        match result {
            Ok(match_ids) => {
                for match_id in match_ids {
                    self.project_match(match_id).await;
                }
            }
            Err(e) => tracing::error!("Error loading finished matches for stats projection: {e}"),
        }
    }

    async fn project_match(&self, match_id: MatchId) {
        match self.stats_repo.has_projected_match(&match_id).await {
            Ok(true) => return,
            Ok(false) => {}
            Err(e) => {
                tracing::error!("Error checking stats projection marker for {match_id:?}: {e}");
                return;
            }
        }

        let mut events = match self.matches_repo.load_events(&match_id).await {
            Ok(events) => events
                .into_iter()
                .map(|dto| dto.event)
                .collect::<Vec<MatchEvent>>(),
            Err(e) => {
                tracing::error!("Error loading events for stats projection {match_id:?}: {e}");
                return;
            }
        };
        let metadata = match self.matches_repo.metadata(&match_id).await {
            Ok(Some(metadata)) => metadata,
            Ok(None) => {
                tracing::error!("Match metadata missing for stats projection {match_id:?}");
                return;
            }
            Err(e) => {
                tracing::error!(
                    "Error loading match metadata for stats projection {match_id:?}: {e}"
                );
                return;
            }
        };

        let mut player_events = metadata
            .players
            .into_iter()
            .map(|player_id| MatchEvent::PlayerJoined {
                user_claims: UserClaims::Anonymous(AnonymousUserClaims {
                    id: PlayerId(player_id.into()),
                    data: serde_json::Value::Null,
                    role: Default::default(),
                }),
            })
            .collect::<Vec<_>>();
        player_events.append(&mut events);
        events = player_events;

        let stats = match project_match_stats(&match_id, &events) {
            Ok(stats) => stats,
            Err(e) => {
                tracing::error!("Error projecting stats for {match_id:?}: {e}");
                return;
            }
        };

        if stats.is_empty() {
            tracing::error!("Stats projection for {match_id:?} produced no rows");
            return;
        }

        for player_stats in stats {
            match self
                .stats_repo
                .has_match_player_stats(&match_id, &player_stats.player_id)
                .await
            {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => {
                    tracing::error!(
                        "Error checking stats projection for {match_id:?}/{}: {e}",
                        player_stats.player_id
                    );
                    return;
                }
            }

            if let Err(e) = self.stats_repo.insert_match_stats(&player_stats).await {
                tracing::error!(
                    "Error storing match stats for {match_id:?}/{}: {e}",
                    player_stats.player_id
                );
                return;
            }

            if let Err(e) = self.stats_repo.apply_match_stats(&player_stats).await {
                tracing::error!(
                    "Error updating player stats for {match_id:?}/{}: {e}",
                    player_stats.player_id
                );
                return;
            }
        }

        if let Err(e) = self.stats_repo.mark_match_projected(&match_id).await {
            tracing::error!("Error marking stats projection complete for {match_id:?}: {e}");
        }
    }
}
