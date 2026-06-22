use indexmap::IndexMap;

use super::*;

#[derive(Debug)]
pub struct GameData {
    pub players: IndexMap<PlayerId, PlayerGameData>,
    pub mode: DealingMode,
    pub cards_count: usize,
    pub order: CyclicIterator,
    settings: GameSettings,
}

impl GameData {
    pub fn new(players: &[PlayerId], settings: GameSettings) -> Self {
        let order = CyclicIterator::new(players.len());

        let players = players
            .iter()
            .map(|id| {
                let player = PlayerGameData {
                    lifes: settings.lifes,
                };
                (id.clone(), player)
            })
            .collect();

        Self {
            mode: settings.mode,
            cards_count: settings.cards_count,
            players,
            order,
            settings,
        }
    }

    pub fn get_lifes(&self) -> HashMap<PlayerId, usize> {
        self.players
            .iter()
            .map(|(id, player)| (id.clone(), player.lifes))
            .collect()
    }

    pub fn get_current(&self) -> &PlayerId {
        let idx = match self.order.peek() {
            Some(i) => i,
            None => {
                let msg = "InvalidGameState getting bid player";
                tracing::error!(msg);
                panic!("{msg}");
            }
        };

        self.get_player_data_idx(idx).0
    }

    pub fn remove_life(&mut self, player: &PlayerId) {
        let (idx, player) = self.get_player_data_mut(player);

        player.lifes -= 1;

        self.order.remove(idx);
    }

    fn get_player_data_mut(&mut self, player: &PlayerId) -> (usize, &mut PlayerGameData) {
        let (idx, _, player) = self
            .players
            .get_full_mut(player)
            .expect("Player should be here");

        (idx, player)
    }

    pub fn get_player_data(&self, player: &PlayerId) -> (usize, &PlayerGameData) {
        let (idx, _, player) = self
            .players
            .get_full(player)
            .expect("Player should be here");

        (idx, player)
    }

    fn get_player_data_idx(&self, idx: usize) -> (&PlayerId, &PlayerGameData) {
        self.players.get_index(idx).expect("Player should be here")
    }

    pub fn alive_players(&self) -> impl Iterator<Item = (&PlayerId, &PlayerGameData)> {
        self.players.iter().filter(|(_, p)| p.lifes > 0)
    }

    pub fn peek_current(&self) -> Option<&PlayerId> {
        self.order.peek().map(|i| self.get_player_data_idx(i).0)
    }
}

#[derive(Debug)]
struct PlayerGameData {
    pub lifes: usize,
}
