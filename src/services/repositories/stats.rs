use futures::TryStreamExt;
use mongodb::{Collection, Database, IndexModel, bson::doc};

use crate::{
    infra::telemetry,
    models::id::{MatchId, PlayerId},
    services::stats::{MatchPlayerStats, PlayerStats},
};

#[derive(Clone)]
pub struct StatsRepository {
    match_player_stats: Collection<MatchPlayerStats>,
    player_stats: Collection<PlayerStats>,
    projected_matches: Collection<ProjectedMatchStats>,
}

impl StatsRepository {
    pub fn new(database: &Database) -> Self {
        Self {
            match_player_stats: database.collection("MatchPlayerStats"),
            player_stats: database.collection("PlayerStats"),
            projected_matches: database.collection("StatsProjectedMatches"),
        }
    }

    pub async fn ensure_indexes(&self) -> mongodb::error::Result<()> {
        telemetry::db_query("MatchPlayerStats", "create_index", async {
            self.match_player_stats
                .create_index(
                    IndexModel::builder()
                        .keys(doc! { "match_id": 1, "player_id": 1 })
                        .build(),
                )
                .await
        })
        .await?;
        telemetry::db_query("PlayerStats", "create_index.player_id", async {
            self.player_stats
                .create_index(IndexModel::builder().keys(doc! { "player_id": 1 }).build())
                .await
        })
        .await?;
        telemetry::db_query("PlayerStats", "create_index.leaderboard", async {
            self.player_stats
                .create_index(
                    IndexModel::builder()
                        .keys(doc! { "matches_won": -1, "rounds_won": -1, "games_played": 1 })
                        .build(),
                )
                .await
        })
        .await?;
        telemetry::db_query("StatsProjectedMatches", "create_index", async {
            self.projected_matches
                .create_index(IndexModel::builder().keys(doc! { "match_id": 1 }).build())
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn has_projected_match(&self, match_id: &MatchId) -> mongodb::error::Result<bool> {
        telemetry::db_query("StatsProjectedMatches", "find_one", async {
            self.projected_matches
                .find_one(doc! { "match_id": match_id.as_str() })
                .await
        })
        .await
        .map(|stats| stats.is_some())
    }

    pub async fn mark_match_projected(&self, match_id: &MatchId) -> mongodb::error::Result<()> {
        let projected = ProjectedMatchStats {
            match_id: match_id.as_str().to_string(),
        };

        telemetry::db_query("StatsProjectedMatches", "replace_one.upsert", async {
            self.projected_matches
                .replace_one(doc! { "match_id": match_id.as_str() }, &projected)
                .upsert(true)
                .await
        })
        .await?;

        Ok(())
    }

    pub async fn has_match_player_stats(
        &self,
        match_id: &MatchId,
        player_id: &str,
    ) -> mongodb::error::Result<bool> {
        telemetry::db_query("MatchPlayerStats", "find_one", async {
            self.match_player_stats
                .find_one(doc! { "match_id": match_id.as_str(), "player_id": player_id })
                .await
        })
        .await
        .map(|stats| stats.is_some())
    }

    pub async fn insert_match_stats(&self, stats: &MatchPlayerStats) -> mongodb::error::Result<()> {
        telemetry::db_query("MatchPlayerStats", "insert_one", async {
            self.match_player_stats.insert_one(stats).await
        })
        .await?;

        Ok(())
    }

    pub async fn player_stats(
        &self,
        player_id: &PlayerId,
    ) -> mongodb::error::Result<Option<PlayerStats>> {
        telemetry::db_query("PlayerStats", "find_one", async {
            self.player_stats
                .find_one(doc! { "player_id": player_id.as_str() })
                .await
        })
        .await
    }

    pub async fn leaderboard(&self, limit: i64) -> mongodb::error::Result<Vec<PlayerStats>> {
        telemetry::db_query("PlayerStats", "find.leaderboard", async {
            let cursor = self
                .player_stats
                .find(doc! {})
                .sort(doc! { "matches_won": -1, "rounds_won": -1, "games_played": 1 })
                .limit(limit)
                .await?;

            cursor.try_collect().await
        })
        .await
    }

    pub async fn apply_match_stats(&self, stats: &MatchPlayerStats) -> mongodb::error::Result<()> {
        let mut aggregate =
            telemetry::db_query("PlayerStats", "find_one.apply_match_stats", async {
                self.player_stats
                    .find_one(doc! { "player_id": &stats.player_id })
                    .await
            })
            .await?
            .unwrap_or_else(|| PlayerStats::new(stats.player_id.clone()));

        aggregate.apply_match(stats);

        telemetry::db_query("PlayerStats", "replace_one.upsert", async {
            self.player_stats
                .replace_one(doc! { "player_id": &stats.player_id }, &aggregate)
                .upsert(true)
                .await
        })
        .await?;

        Ok(())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ProjectedMatchStats {
    match_id: String,
}
