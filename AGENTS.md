# AGENTS.md

## Fast Start

- This is a single-crate Rust backend for the Fodinha card game. The process starts in `src/main.rs`: init telemetry, load `AppSettings`, start `GameManager`, then serve Axum.
- Run Mongo first: `podman compose up -d mongodb`
- Run the API: `cargo run`
- `JWT_KEY` has no default and is required for the repo's auth flows. `MONGO_CONN_STRING`, `MONGO_DATABASE`, and `GOOGLE_CLIENT_ID` are loaded from env with defaults in `src/lib.rs`.
- There is no CI test/lint workflow in this repo right now; `.github/workflows/fly-deploy.yml` only deploys to Fly on push to `main`.

## Verification

- `cargo test` uses a real MongoDB at `mongodb://localhost/?retryWrites=true` and creates per-test databases named `oh_hell_test_*` inside `src/infra/api/mod.rs` tests.
- Good focused regression check for actor restore/replay work: `cargo test test_concurrent_lazy_loads_match_actor_from_events -- --exact`
- Good focused regression check for persistence boundaries: `cargo test test_lobby_changes_do_not_write_match_events -- --exact`

## Architecture

- `src/services/matches/manager.rs` is the main coordinator. `ManagerHandle` is what HTTP and WebSocket handlers call. It owns the in-memory registry of match actors and lazy-loads missing actors from Mongo.
- `src/services/matches/actor.rs` is the single-writer actor for one match/lobby. Route every game-state mutation through `MatchActorMessage`; do not update lobby/game state from API handlers or repositories.
- Concurrency detail that matters: actor lookup uses `DashMap` plus a `watch` channel in `src/services/matches/registry.rs` so concurrent requests for the same unloaded match collapse into one restore path.
- Cold restore path: `ManagerHandle::sender_for_match()` -> `load_match_actor()` -> load `MatchMetadata` + ordered `MatchEvents` -> `restore_from_metadata()` -> `replay_event()` -> spawn actor.

## Durability Split

- This repo is not using one storage mechanism for everything.
- `MatchMetadata` is the current-room projection used for lobby listing, player routing, ready flags, and restoring waiting matches.
- `MatchEvents` is the append-only event stream used for replaying gameplay state and post-game stats projection.
- Important current behavior: lobby create/join/ready changes are persisted to `MatchMetadata`, but do not write `MatchEvents`. This is verified by `test_lobby_changes_do_not_write_match_events`.
- Gameplay events are appended from `MatchActor::persist_apply()`. If you change event shapes or ordering, audit replay in `actor.rs`, metadata projection in `src/services/matches/projection.rs`, and stats projection in `src/services/stats/` together.

## API Boundaries

- HTTP route wiring lives in `src/infra/api/mod.rs`.
- `/lobby` is authenticated by Bearer middleware.
- `/game` is a WebSocket endpoint that authenticates with `?token=...`, not an `Authorization` header.
- A player must join a lobby before opening `/game`; websocket connection resolution depends on `player_routes` in memory or `active_metadata_for_player()` in Mongo.
- `src/models/commands.rs` is the wire contract for REST/WS payloads. Check it before changing client-visible message shapes.

## Directory Map

- `src/models/game/mod.rs`: core Fodinha rules, dealing/bidding/turn logic, and `MatchEvent` definitions.
- `src/models/lobby.rs`: waiting-vs-playing snapshots and per-player lobby state.
- `src/services/repositories/`: direct Mongo access. Collection names are hard-coded here.
- `src/services/stats/projector.rs`: background projector that re-reads finished match events and updates `MatchPlayerStats`, `PlayerStats`, and projection markers.
- `src/infra/telemetry.rs`: `/metrics`, HTTP/DB/actor metrics, optional OTLP export. Fly/Grafana config lives in `fly.toml`, `monitoring/`, and `scripts/import-grafana-dashboard.mjs`.

## Change Traps

- If you change player identity/auth, audit `src/infra/api/auth.rs`, `src/services/repositories/users.rs`, fallback user hydration in `src/services/matches/manager.rs`, and stats response hydration.
- If you change match finish behavior, also audit actor cleanup (`stop_match()`), metadata status updates, and `StatsProjectorHandle::notify_match_finished()`.
- Stats are eventually consistent by design: finished matches trigger background projection after the actor stops, so stats endpoints may need polling in tests and clients.
