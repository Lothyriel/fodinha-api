use std::collections::HashMap;

use indexmap::IndexMap;

use crate::{
    infra::UserClaims,
    models::{
        Card, Turn,
        game::{
            GameEvent, MatchEvent,
            fodinha_classic::{self, AppliedGameChange, GameOutcome},
        },
        id::{MatchId, PlayerId},
    },
};

use super::{MatchPlayerStats, card_key};

#[derive(Debug, thiserror::Error)]
pub(crate) enum StatsProjectionError {
    #[error("match has game event before game start")]
    GameNotStarted,
    #[error("match is missing current upcard")]
    MissingUpcard,
    #[error("invalid game start: {0}")]
    InvalidGameStart(#[from] crate::models::GameError),
    #[error("unexpected game application result")]
    UnexpectedAppliedChange,
}

pub(crate) fn project_match_stats(
    match_id: &MatchId,
    events: &[MatchEvent],
) -> Result<Vec<MatchPlayerStats>, StatsProjectionError> {
    let mut players = IndexMap::<PlayerId, UserClaims>::new();
    let mut stats = HashMap::<PlayerId, MatchPlayerStats>::new();
    let mut game = None;
    let mut current_upcard = None;
    let mut current_bids = HashMap::<PlayerId, usize>::new();
    let mut current_rounds = HashMap::<PlayerId, usize>::new();
    let mut finished_lifes = None;

    for event in events {
        match event {
            MatchEvent::MatchCreated { .. } | MatchEvent::PlayerStatusChanged { .. } => {}
            MatchEvent::PlayerJoined { user_claims } => {
                let player_id = user_claims.id();
                players.insert(player_id.clone(), user_claims.clone());
                ensure_player_stats(&mut stats, match_id, &player_id);
            }
            MatchEvent::Game(GameEvent::FodinhaClassic(
                fodinha_classic::MatchEvent::GameStarted { settings, set },
            )) => {
                let player_ids = match players.is_empty() {
                    true => set.decks.keys().cloned().collect::<Vec<_>>(),
                    false => players.keys().cloned().collect::<Vec<_>>(),
                };

                for player_id in &player_ids {
                    ensure_player_stats(&mut stats, match_id, player_id);
                }

                game = Some(fodinha_classic::Game::from_started(
                    &player_ids,
                    settings.clone(),
                    set.clone(),
                )?);
                current_upcard = Some(set.upcard);
                current_bids.clear();
                current_rounds.clear();
                add_dealt_trumps(&mut stats, match_id, &set.decks, set.upcard);
            }
            MatchEvent::Game(GameEvent::FodinhaClassic(
                event @ fodinha_classic::MatchEvent::BidPlaced { player_id, bid },
            )) => {
                let game = game.as_mut().ok_or(StatsProjectionError::GameNotStarted)?;
                let AppliedGameChange::BidPlaced { .. } = game.apply_match_event(event.clone())
                else {
                    return Err(StatsProjectionError::UnexpectedAppliedChange);
                };

                let player_stats = ensure_player_stats(&mut stats, match_id, player_id);
                player_stats.total_bid += *bid as i64;
                player_stats.bid_count += 1;
                current_bids.insert(player_id.clone(), *bid);
            }
            MatchEvent::Game(GameEvent::FodinhaClassic(
                event @ fodinha_classic::MatchEvent::TurnPlayed { .. },
            )) => {
                let upcard = current_upcard.ok_or(StatsProjectionError::MissingUpcard)?;
                let game = game.as_mut().ok_or(StatsProjectionError::GameNotStarted)?;
                let AppliedGameChange::TurnPlayed(state) = game.apply_match_event(event.clone())
                else {
                    return Err(StatsProjectionError::UnexpectedAppliedChange);
                };

                match state.outcome {
                    GameOutcome::TurnPlayed { .. } => {}
                    GameOutcome::RoundEnded { next, .. } => {
                        record_round_win(
                            &mut stats,
                            match_id,
                            &mut current_rounds,
                            &next,
                            &state.pile,
                            upcard,
                        );
                    }
                    GameOutcome::SetEnded {
                        upcard: next_upcard,
                        decks,
                        ..
                    } => {
                        if let Some(winner) = winning_turn(&state.pile, upcard) {
                            let winner = winner.player_id.clone();
                            record_round_win(
                                &mut stats,
                                match_id,
                                &mut current_rounds,
                                &winner,
                                &state.pile,
                                upcard,
                            );
                        }

                        finish_set(&mut stats, match_id, &current_bids, &current_rounds);
                        current_bids.clear();
                        current_rounds.clear();
                        current_upcard = Some(next_upcard);
                        add_dealt_trumps(&mut stats, match_id, &decks, next_upcard);
                    }
                    GameOutcome::Ended { lifes } => {
                        if let Some(winner) = winning_turn(&state.pile, upcard) {
                            let winner = winner.player_id.clone();
                            record_round_win(
                                &mut stats,
                                match_id,
                                &mut current_rounds,
                                &winner,
                                &state.pile,
                                upcard,
                            );
                        }

                        finish_set(&mut stats, match_id, &current_bids, &current_rounds);
                        finished_lifes = Some(lifes);
                    }
                }
            }
            MatchEvent::Game(GameEvent::FodinhaPower(_)) => {}
        }
    }

    let Some(lifes) = finished_lifes else {
        return Ok(Vec::new());
    };

    for player_stats in stats.values_mut() {
        player_stats.games_played = 1;
    }

    let max_life = lifes.values().copied().max().unwrap_or_default();

    if max_life > 0 {
        for (player_id, life) in lifes {
            if life == max_life {
                ensure_player_stats(&mut stats, match_id, &player_id).matches_won = 1;
            }
        }
    }

    Ok(stats.into_values().filter(|s| s.games_played > 0).collect())
}

fn add_dealt_trumps(
    stats: &mut HashMap<PlayerId, MatchPlayerStats>,
    match_id: &MatchId,
    decks: &IndexMap<PlayerId, Vec<Card>>,
    upcard: Card,
) {
    for (player_id, deck) in decks {
        let trump_cards = deck.iter().filter(|card| card.is_trump(upcard)).count() as i64;
        ensure_player_stats(stats, match_id, player_id).trump_cards += trump_cards;
    }
}

fn record_round_win(
    stats: &mut HashMap<PlayerId, MatchPlayerStats>,
    match_id: &MatchId,
    current_rounds: &mut HashMap<PlayerId, usize>,
    winner: &PlayerId,
    pile: &[Turn],
    upcard: Card,
) {
    let winning_turn = pile
        .iter()
        .find(|turn| &turn.player_id == winner)
        .or_else(|| winning_turn(pile, upcard));

    let player_stats = ensure_player_stats(stats, match_id, winner);
    player_stats.rounds_won += 1;
    *current_rounds.entry(winner.clone()).or_default() += 1;

    if let Some(turn) = winning_turn {
        *player_stats
            .winning_cards
            .entry(card_key(turn.card))
            .or_default() += 1;
    }
}

fn finish_set(
    stats: &mut HashMap<PlayerId, MatchPlayerStats>,
    match_id: &MatchId,
    current_bids: &HashMap<PlayerId, usize>,
    current_rounds: &HashMap<PlayerId, usize>,
) {
    for (player_id, bid) in current_bids {
        let rounds = current_rounds.get(player_id).copied().unwrap_or_default();
        let player_stats = ensure_player_stats(stats, match_id, player_id);

        if rounds == *bid {
            player_stats.bids_hit += 1;
        } else {
            player_stats.bids_missed += 1;
        }
    }
}

fn ensure_player_stats<'a>(
    stats: &'a mut HashMap<PlayerId, MatchPlayerStats>,
    match_id: &MatchId,
    player_id: &PlayerId,
) -> &'a mut MatchPlayerStats {
    stats.entry(player_id.clone()).or_insert_with(|| {
        MatchPlayerStats::new(
            match_id.as_str().to_string(),
            player_id.as_str().to_string(),
        )
    })
}

fn winning_turn(pile: &[Turn], upcard: Card) -> Option<&Turn> {
    pile.iter()
        .max_by_key(|turn| turn.card.get_trump_value(upcard))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        infra::{AnonymousUserClaims, UserClaims},
        models::{
            Card, Rank, Suit,
            game::fodinha_classic::{
                DealingMode, DeckShuffle, GameSettings, MatchEvent as FodinhaEvent, NewSet,
            },
            id::{LobbyId, PlayerId},
        },
    };

    use super::*;

    #[test]
    fn projects_finished_match_player_stats() {
        let match_id = LobbyId(Arc::from("match-1"));
        let player1 = PlayerId(Arc::from("P1"));
        let player2 = PlayerId(Arc::from("P2"));
        let upcard = Card::new(Rank::Three, Suit::Clubs);
        let winning_card = Card::new(Rank::Four, Suit::Golds);
        let losing_card = Card::new(Rank::Five, Suit::Golds);
        let events = vec![
            MatchEvent::MatchCreated {
                settings: crate::models::game::GameSettings::FodinhaClassic(GameSettings {
                    lifes: 1,
                }),
            },
            MatchEvent::PlayerJoined {
                user_claims: anonymous(&player1, "Player 1"),
            },
            MatchEvent::PlayerJoined {
                user_claims: anonymous(&player2, "Player 2"),
            },
            MatchEvent::Game(GameEvent::FodinhaClassic(FodinhaEvent::GameStarted {
                settings: GameSettings { lifes: 1 },
                set: NewSet {
                    dealing_mode: DealingMode::Increasing,
                    cards_count: 1,
                    shuffle: DeckShuffle {
                        seed: 1,
                        sequence: 0,
                    },
                    decks: indexmap::indexmap! {
                        player1.clone() => vec![winning_card],
                        player2.clone() => vec![losing_card],
                    },
                    upcard,
                },
            })),
            MatchEvent::Game(GameEvent::FodinhaClassic(FodinhaEvent::BidPlaced {
                player_id: player1.clone(),
                bid: 1,
            })),
            MatchEvent::Game(GameEvent::FodinhaClassic(FodinhaEvent::BidPlaced {
                player_id: player2.clone(),
                bid: 1,
            })),
            MatchEvent::Game(GameEvent::FodinhaClassic(FodinhaEvent::TurnPlayed {
                turn: Turn {
                    player_id: player1.clone(),
                    card: winning_card,
                },
                next_set: None,
            })),
            MatchEvent::Game(GameEvent::FodinhaClassic(FodinhaEvent::TurnPlayed {
                turn: Turn {
                    player_id: player2.clone(),
                    card: losing_card,
                },
                next_set: None,
            })),
        ];

        let stats = project_match_stats(&match_id, &events).unwrap();
        let player1_stats = stats
            .iter()
            .find(|stats| stats.player_id == player1.as_str())
            .unwrap();
        let player2_stats = stats
            .iter()
            .find(|stats| stats.player_id == player2.as_str())
            .unwrap();

        assert_eq!(player1_stats.games_played, 1);
        assert_eq!(player1_stats.matches_won, 1);
        assert_eq!(player1_stats.rounds_won, 1);
        assert_eq!(player1_stats.trump_cards, 1);
        assert_eq!(player1_stats.total_bid, 1);
        assert_eq!(player1_stats.bids_hit, 1);
        assert_eq!(player2_stats.games_played, 1);
        assert_eq!(player2_stats.matches_won, 0);
        assert_eq!(player2_stats.bids_hit, 0);
        assert_eq!(player2_stats.bids_missed, 1);
    }

    fn anonymous(player_id: &PlayerId, nickname: &str) -> UserClaims {
        UserClaims::Anonymous(AnonymousUserClaims {
            id: player_id.clone(),
            data: serde_json::json!({ "nickname": nickname }),
            role: Default::default(),
        })
    }
}
