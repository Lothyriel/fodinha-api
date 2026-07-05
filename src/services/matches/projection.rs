use crate::{
    models::{
        game::{GameEvent, MatchEvent, fodinha_classic},
        id::MatchId,
    },
    services::repositories::matches::MatchesRepository,
};

pub(crate) async fn project_match_metadata(
    repo: &MatchesRepository,
    match_id: &MatchId,
    event: &MatchEvent,
    match_finished: bool,
) -> mongodb::error::Result<()> {
    match event {
        MatchEvent::MatchCreated { settings } => {
            repo.create_metadata(match_id, settings.clone(), None).await
        }
        MatchEvent::PlayerJoined { user_claims } => {
            let player_id = user_claims.id();

            repo.add_metadata_player(match_id, &player_id).await
        }
        MatchEvent::Game(GameEvent::FodinhaClassic(fodinha_classic::MatchEvent::GameStarted {
            ..
        })) => repo.mark_metadata_playing(match_id).await,
        MatchEvent::Game(GameEvent::FodinhaClassic(fodinha_classic::MatchEvent::TurnPlayed {
            ..
        })) if match_finished => repo.mark_metadata_finished(match_id).await,
        _ => Ok(()),
    }
}
