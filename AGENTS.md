# AGENTS.md

## Fast Start

- This is a Rust workspace for a card-game platform with two crates: the main `fodinha-api` API crate and the `power-lua-api` library crate under `crates/power-lua-api/`. The only supported game type today is `fodinha_classic`, implemented under the generic game facade in `src/models/game/`.
- The process starts in `src/main.rs`: init telemetry, load `AppSettings`, start `GameManager`, then serve Axum.
- Run Mongo first: `podman compose up -d mongodb`
- Run the API: `cargo run`
- `JWT_KEY` has no default and is required for the repo's auth flows. `MONGO_CONN_STRING`, `MONGO_DATABASE`, and `GOOGLE_CLIENT_ID` are loaded from env with defaults in `src/lib.rs`.
- There is no CI test/lint workflow in this repo right now; `.github/workflows/fly-deploy.yml` only deploys to Fly on push to `main`.

## Verification

- After every feature change, run the relevant backend and frontend tests before considering the change complete. Report any failing tests and do not treat a build-only pass as sufficient verification.
- Run workspace commands from `api/`; `cargo test` runs tests for both crates. The API integration tests use a real MongoDB at `mongodb://localhost/?retryWrites=true` and create per-test databases named `oh_hell_test_*` inside `src/infra/api/mod.rs` tests.
- `power-lua-api` generates `fodinha.d.lua`, `power-card-template.lua`, and `mercenary-passive-template.lua` into Cargo's `OUT_DIR`; the API embeds those generated files at compile time.
- Good focused regression check for actor restore/replay work: `cargo test test_concurrent_lazy_loads_match_actor_from_events -- --exact`
- Good focused regression check for persistence boundaries: `cargo test test_lobby_changes_do_not_write_match_events -- --exact`
- Good focused check for game event BSON/wire serialization: `cargo test game::tests`

## Architecture

- `src/services/matches/manager.rs` is the main coordinator. `ManagerHandle` is what HTTP and WebSocket handlers call. It owns the in-memory registry of match actors and lazy-loads missing actors from Mongo.
- `src/services/matches/actor.rs` is the single-writer actor for one match/lobby. Route every game-state mutation through `MatchActorMessage`; do not update lobby/game state from API handlers or repositories.
- Concurrency detail that matters: actor lookup uses `DashMap` plus a `watch` channel in `src/services/matches/registry.rs` so concurrent requests for the same unloaded match collapse into one restore path.
- Cold restore path: `ManagerHandle::sender_for_match()` -> `load_match_actor()` -> load `MatchMetadata` + ordered `MatchEvents` -> `restore_from_metadata()` -> `replay_event()` -> spawn actor.
- Game type boundary: `src/models/game/mod.rs` owns the generic facade: `GameType`, typed `GameSettings`, typed `GameCommand`, typed `GameEvent`, the generic `Game` enum, `LobbyState`, and top-level `MatchEvent` common events.
- Current rules live in `src/models/game/fodinha_classic.rs`: bidding/dealing/turn logic plus Fodinha-specific settings, commands, events, and outcomes. Do not put generic multi-game dispatch logic in this file.
- `fodinha_classic::GameSettings` intentionally contains only user-configurable settings (`lifes`). Round state such as `cards_count` and `dealing_mode` belongs in `NewSet`/`Game`, starts at 1 card and increasing mode, and is persisted only through gameplay events.
- Adding a new game type means adding a module beside `fodinha_classic`, adding variants to the facade enums in `src/models/game/mod.rs`, and wiring dispatch for start, validate command, apply event, snapshot/game info, and finish checks.

## Durability Split

- This repo is not using one storage mechanism for everything.
- `MatchMetadata` is the current-room projection used for lobby listing, player routing, ready flags, and restoring waiting matches.
- `MatchEvents` is the append-only event stream used for replaying gameplay state and post-game stats projection.
- `MatchMetadata.settings` stores typed settings as `{ game_type, settings }`; for `fodinha_classic`, `settings` should only contain `lifes`. `MatchEvents.event` stores common lobby events directly and gameplay events as `MatchEvent::Game(GameEvent::...)` with a nested `game_type`.
- Important current behavior: lobby create/join/ready changes are persisted to `MatchMetadata`, but do not write `MatchEvents`. This is verified by `test_lobby_changes_do_not_write_match_events`.
- Gameplay events are appended from `MatchActor::persist_apply()`. If you change event shapes or ordering, audit replay in `actor.rs`, metadata projection in `src/services/matches/projection.rs`, and stats projection in `src/services/stats/` together.

## API Boundaries

- HTTP route wiring lives in `src/infra/api/mod.rs`.
- `/lobby` is authenticated by Bearer middleware.
- `POST /lobby` requires `game_type`. Fodinha-specific `lifes` is accepted at the top level, e.g. `{ "game_type": "fodinha_classic", "lifes": 5 }`.
- `CreateLobbyResponse` returns `{ lobby_id, game_type }`; `GET /lobby` returns `GetLobbyDto` entries with `{ id, game_type, player_count }`.
- `/game` is a WebSocket endpoint that authenticates with `?token=...`, not an `Authorization` header.
- A player must join a lobby before opening `/game`; websocket connection resolution depends on `player_routes` in memory or `active_metadata_for_player()` in Mongo.
- `src/models/commands.rs` is the wire contract for REST/WS payloads. Check it before changing client-visible message shapes.
- WebSocket `PlayerStatusChange` is a common lobby command. Gameplay commands must use the typed envelope: `{ "type": "GameCommand", "data": { "game_type": "fodinha_classic", "command": { "type": "PutBid", "data": { "bid": 1 } } } }` or the same envelope with `PlayTurn`.
- `MatchActor` validates `GameCommand.game_type()` against the lobby's active game type before applying the command. Wrong-game commands should be rejected before event persistence.

## Directory Map

- `src/models/game/mod.rs`: generic game facade and typed game/lobby event boundary.
- `src/models/game/fodinha_classic.rs`: current Fodinha Classic rules, settings, commands, events, dealing/bidding/turn logic.
- `src/models/commands.rs`: REST/WS payload contracts shared with clients.
- `src/models/lobby.rs`: waiting-vs-playing snapshots and per-player lobby state.
- `src/services/repositories/`: direct Mongo access. Collection names are hard-coded here.
- `src/services/stats/projector.rs`: background projector that re-reads finished match events and updates `MatchPlayerStats`, `PlayerStats`, and projection markers.
- `src/infra/telemetry.rs`: `/metrics`, HTTP/DB/actor metrics, optional OTLP export. Fly/Grafana config lives in `fly.toml`, `monitoring/`, and `scripts/import-grafana-dashboard.mjs`.

## Change Traps

- If you change player identity/auth, audit `src/infra/api/auth.rs`, `src/services/repositories/users.rs`, fallback user hydration in `src/services/matches/manager.rs`, and stats response hydration.
- If you change match finish behavior, also audit actor cleanup (`stop_match()`), metadata status updates, and `StatsProjectorHandle::notify_match_finished()`.
- Stats projection is currently Fodinha-specific. If adding a new game type, either implement a game-specific stats projector path or explicitly skip stats for that game.
- If you change `GameType`, `GameSettings`, `GameCommand`, `GameEvent`, or `MatchEvent` serialization, run `cargo test game::tests` and at least one restore/replay regression test.
- Stats are eventually consistent by design: finished matches trigger background projection after the actor stops, so stats endpoints may need polling in tests and clients.
