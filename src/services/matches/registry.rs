use std::sync::Arc;

use dashmap::{DashMap, mapref::entry::Entry};
use tokio::sync::watch;

use crate::{
    models::id::{MatchId, PlayerId},
    services::ManagerError,
};

use super::MatchSender;

pub type MatchEntries = Arc<DashMap<MatchId, MatchEntry>>;
pub type PlayerRoutes = Arc<DashMap<PlayerId, MatchId>>;

#[derive(Clone)]
pub enum MatchEntry {
    Loading(Arc<watch::Sender<Option<MatchSender>>>),
    Ready(MatchSender),
}

#[derive(Clone)]
pub(crate) struct MatchRegistry {
    pub(crate) matches: MatchEntries,
    pub(crate) player_routes: PlayerRoutes,
}

impl MatchRegistry {
    pub(crate) fn new() -> Self {
        Self {
            matches: Arc::new(DashMap::new()),
            player_routes: Arc::new(DashMap::new()),
        }
    }

    #[cfg(test)]
    pub(crate) fn player_route_count(&self) -> usize {
        self.player_routes.len()
    }

    pub(crate) fn mark_ready(&self, match_id: MatchId, sender: MatchSender) {
        self.matches.insert(match_id, MatchEntry::Ready(sender));
    }

    pub(crate) fn remove_match(&self, match_id: &MatchId) {
        self.matches.remove(match_id);
    }

    pub(crate) fn match_for_player(&self, player_id: &PlayerId) -> Option<MatchId> {
        self.player_routes
            .get(player_id)
            .map(|entry| entry.value().clone())
    }

    pub(crate) async fn sender_or_mark_loading(
        &self,
        match_id: &MatchId,
    ) -> Result<SenderLookup, ManagerError> {
        loop {
            match self.matches.entry(match_id.clone()) {
                Entry::Occupied(entry) => match entry.get() {
                    MatchEntry::Ready(sender) => return Ok(SenderLookup::Ready(sender.clone())),
                    MatchEntry::Loading(loading) => {
                        let mut rx = loading.subscribe();

                        if let Some(sender) = rx.borrow().clone() {
                            return Ok(SenderLookup::Ready(sender));
                        }

                        drop(entry);

                        if rx.changed().await.is_err() {
                            continue;
                        }

                        if let Some(sender) = rx.borrow().clone() {
                            return Ok(SenderLookup::Ready(sender));
                        }
                    }
                },
                Entry::Vacant(entry) => {
                    let (tx, _) = watch::channel(None);
                    let loading = Arc::new(tx);
                    entry.insert(MatchEntry::Loading(loading.clone()));

                    return Ok(SenderLookup::Load(loading));
                }
            }
        }
    }

    pub(crate) fn finish_loading(
        &self,
        match_id: &MatchId,
        loading: &Arc<watch::Sender<Option<MatchSender>>>,
        result: &Result<MatchSender, ManagerError>,
    ) {
        match result {
            Ok(sender) => {
                let _ = loading.send(Some(sender.clone()));
            }
            Err(_) => {
                self.remove_loading_entry(match_id, loading);
            }
        }
    }

    fn remove_loading_entry(
        &self,
        match_id: &MatchId,
        loading: &Arc<watch::Sender<Option<MatchSender>>>,
    ) {
        let should_remove = self.matches.get(match_id).is_some_and(|entry| {
            matches!(entry.value(), MatchEntry::Loading(current) if Arc::ptr_eq(current, loading))
        });

        if should_remove {
            self.matches.remove(match_id);
        }
    }
}

pub(crate) enum SenderLookup {
    Ready(MatchSender),
    Load(Arc<watch::Sender<Option<MatchSender>>>),
}
