use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rand::RngExt;
use tokio::{sync::mpsc, time::MissedTickBehavior};

use oh_hell::client::{ClientError, GameOutcome, GameSession, HttpClient, TurnDelay, WsClient};

#[derive(serde::Deserialize, Clone)]
struct Config {
    host: String,
    min_players: usize,
    max_players: usize,
    min_turn_delay_ms: u64,
    max_turn_delay_ms: u64,
    min_bid_delay_ms: u64,
    max_bid_delay_ms: u64,
    #[serde(default)]
    spawn_rate_per_sec: usize,
    #[serde(default)]
    ramp: Vec<RampStep>,
}

#[derive(serde::Deserialize, Clone)]
struct RampStep {
    duration_secs: u64,
    target_games: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: "http://localhost:3000".into(),
            min_players: 4,
            max_players: 10,
            min_turn_delay_ms: 5_000,
            max_turn_delay_ms: 20_000,
            min_bid_delay_ms: 5_000,
            max_bid_delay_ms: 20_000,
            spawn_rate_per_sec: 5,
            ramp: vec![],
        }
    }
}

#[derive(Clone, Debug)]
struct GameResult {
    player_count: usize,
    outcome: String,
    info: String,
}

enum RunnerCmd {
    SetDesired,
    GameFinished(GameResult),
    Shutdown,
}

struct SharedState {
    desired_games: AtomicUsize,
    running_games: AtomicUsize,
    finished_games: AtomicUsize,
    error_count: AtomicUsize,
    shutdown: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct UserPool {
    available: Vec<String>,
    next_id: usize,
}

impl UserPool {
    fn new() -> Self {
        Self {
            available: Vec::new(),
            next_id: 1,
        }
    }

    fn put_back(&mut self, tokens: Vec<String>) {
        self.available.extend(tokens);
    }
}

#[tokio::main]
async fn main() {
    let config = load_config();

    if config.ramp.is_empty() {
        eprintln!("No ramp steps configured. Add a 'ramp' array to config.json.");
        return;
    }

    let ws_url = config
        .host
        .replace("http://", "ws://")
        .replace("https://", "wss://");

    let state = Arc::new(SharedState {
        desired_games: AtomicUsize::new(0),
        running_games: AtomicUsize::new(0),
        finished_games: AtomicUsize::new(0),
        error_count: AtomicUsize::new(0),
        shutdown: AtomicBool::new(false),
    });

    let pool = Arc::new(Mutex::new(UserPool::new()));
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<RunnerCmd>();

    let runner_state = state.clone();
    let runner_config = config.clone();
    let runner_http = HttpClient::new(config.host.clone());
    let runner_ws = WsClient::new(ws_url);
    let runner_tx = cmd_tx.clone();
    let runner_pool = pool.clone();

    tokio::spawn(async move {
        run_game_manager(
            runner_state,
            runner_config,
            runner_http,
            runner_ws,
            runner_tx,
            runner_pool,
            &mut cmd_rx,
        )
        .await;
    });

    let start = Instant::now();
    let ramp = config.ramp.clone();
    let state_clone = state.clone();
    let cmd_tx_clone = cmd_tx.clone();

    tokio::spawn(async move {
        for step in &ramp {
            state_clone
                .desired_games
                .store(step.target_games, Ordering::Relaxed);
            let _ = cmd_tx_clone.send(RunnerCmd::SetDesired);

            eprintln!(
                "[{:>4}s] RAMP -> target={} (hold for {}s)",
                start.elapsed().as_secs(),
                step.target_games,
                step.duration_secs,
            );

            tokio::time::sleep(Duration::from_secs(step.duration_secs)).await;
        }

        eprintln!(
            "[{:>4}s] RAMP complete — holding at final target indefinitely",
            start.elapsed().as_secs(),
        );
    });

    let state_for_status = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;

            if state_for_status.shutdown.load(Ordering::Relaxed) {
                break;
            }

            let desired = state_for_status.desired_games.load(Ordering::Relaxed);
            let running = state_for_status.running_games.load(Ordering::Relaxed);
            let finished = state_for_status.finished_games.load(Ordering::Relaxed);
            let errors = state_for_status.error_count.load(Ordering::Relaxed);

            eprintln!(
                "[{:>4}s] desired={:<6} running={:<6} finished={:<6} errors={}",
                start.elapsed().as_secs(),
                desired,
                running,
                finished,
                errors,
            );
        }
    });

    match tokio::signal::ctrl_c().await {
        Ok(()) => {
            eprintln!("\n--- shutting down ---");
            state.shutdown.store(true, Ordering::Relaxed);
            let _ = cmd_tx.send(RunnerCmd::Shutdown);
        }
        Err(e) => {
            eprintln!("Failed to listen for Ctrl+C: {e}");
        }
    }

    tokio::time::sleep(Duration::from_millis(500)).await;

    let desired = state.desired_games.load(Ordering::Relaxed);
    let running = state.running_games.load(Ordering::Relaxed);
    let finished = state.finished_games.load(Ordering::Relaxed);
    let errors = state.error_count.load(Ordering::Relaxed);

    eprintln!("--- final ---");
    eprintln!(
        "  elapsed={:>4}s  desired={:<6}  running={:<6}  finished={:<6}  errors={}",
        start.elapsed().as_secs(),
        desired,
        running,
        finished,
        errors
    );
}

fn load_config() -> Config {
    let config_path = "config.json";
    match std::fs::read_to_string(config_path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            eprintln!("Invalid config.json ({e}), using defaults");
            Config::default()
        }),
        Err(_) => {
            eprintln!("No config.json found, using defaults");
            Config::default()
        }
    }
}

async fn run_game_manager(
    state: Arc<SharedState>,
    config: Config,
    http: HttpClient,
    ws: WsClient,
    cmd_tx: mpsc::UnboundedSender<RunnerCmd>,
    pool: Arc<Mutex<UserPool>>,
    cmd_rx: &mut mpsc::UnboundedReceiver<RunnerCmd>,
) {
    let spawn_interval = if config.spawn_rate_per_sec == 0 {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(1.0 / config.spawn_rate_per_sec as f64)
    };

    if spawn_interval > Duration::ZERO {
        let mut spawn_tick = tokio::time::interval(spawn_interval);
        spawn_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            if state.shutdown.load(Ordering::Relaxed) {
                return;
            }

            tokio::select! {
                _ = spawn_tick.tick() => {
                    maybe_spawn_game(
                        state.clone(),
                        config.clone(),
                        http.clone(),
                        ws.clone(),
                        cmd_tx.clone(),
                        pool.clone(),
                    );
                }
                msg = cmd_rx.recv() => {
                    handle_runner_cmd(&state, msg);
                }
            }
        }
    }

    loop {
        if state.shutdown.load(Ordering::Relaxed) {
            return;
        }

        if state.desired_games.load(Ordering::Relaxed) > state.running_games.load(Ordering::Relaxed)
        {
            maybe_spawn_game(
                state.clone(),
                config.clone(),
                http.clone(),
                ws.clone(),
                cmd_tx.clone(),
                pool.clone(),
            );
        } else {
            handle_runner_cmd(&state, cmd_rx.recv().await);
        }
    }
}

fn maybe_spawn_game(
    state: Arc<SharedState>,
    config: Config,
    http: HttpClient,
    ws: WsClient,
    cmd_tx: mpsc::UnboundedSender<RunnerCmd>,
    pool: Arc<Mutex<UserPool>>,
) {
    let desired = state.desired_games.load(Ordering::Relaxed);
    let running = state.running_games.load(Ordering::Relaxed);

    if desired <= running {
        return;
    }

    state.running_games.fetch_add(1, Ordering::Relaxed);

    tokio::spawn(async move {
        let player_count = {
            let mut rng = rand::rng();
            rng.random_range(config.min_players..=config.max_players)
        };

        let tokens = match acquire_tokens(&pool, &http, player_count).await {
            Ok(tokens) => tokens,
            Err(e) => {
                let msg = e.to_string();
                log_error(&msg);

                state.error_count.fetch_add(1, Ordering::Relaxed);
                let _ = cmd_tx.send(RunnerCmd::GameFinished(GameResult {
                    player_count,
                    outcome: "ERROR".into(),
                    info: String::new(),
                }));
                state.running_games.fetch_sub(1, Ordering::Relaxed);
                return;
            }
        };

        match run_single_game(&tokens, &http, &ws, &config).await {
            Ok(result) => {
                let _ = cmd_tx.send(RunnerCmd::GameFinished(result));
            }
            Err(e) => {
                let msg = e.to_string();
                log_error(&msg);

                state.error_count.fetch_add(1, Ordering::Relaxed);
                let _ = cmd_tx.send(RunnerCmd::GameFinished(GameResult {
                    player_count,
                    outcome: "ERROR".into(),
                    info: String::new(),
                }));
            }
        }

        let mut guard = lock(&*pool);
        guard.put_back(tokens);
        state.running_games.fetch_sub(1, Ordering::Relaxed);
    });
}

fn handle_runner_cmd(state: &SharedState, msg: Option<RunnerCmd>) {
    match msg {
        Some(RunnerCmd::GameFinished(result)) => {
            state.finished_games.fetch_add(1, Ordering::Relaxed);

            if result.outcome == "OK" {
                println!("OK  {:>2}P | {}", result.player_count, result.info,);
            }
        }
        Some(RunnerCmd::Shutdown) => {
            state.shutdown.store(true, Ordering::Relaxed);
        }
        Some(RunnerCmd::SetDesired) | None => {}
    }
}

async fn acquire_tokens(
    pool: &Arc<Mutex<UserPool>>,
    http: &HttpClient,
    player_count: usize,
) -> Result<Vec<String>, ClientError> {
    let mut tokens = {
        let mut guard = lock(&**pool);
        let available = player_count.min(guard.available.len());

        guard.available.drain(..available).collect::<Vec<_>>()
    };
    let need = player_count.saturating_sub(tokens.len());

    for _ in 0..need {
        let nickname = {
            let mut guard = lock(&**pool);
            let name = format!("Bot{}", guard.next_id);
            guard.next_id += 1;
            name
        };

        match http.try_signup(&nickname).await {
            Ok(token) => tokens.push(token),
            Err(e) => {
                let mut guard = lock(&**pool);
                guard.put_back(tokens);
                return Err(e);
            }
        }
    }

    Ok(tokens)
}

async fn run_single_game(
    tokens: &[String],
    http: &HttpClient,
    ws: &WsClient,
    config: &Config,
) -> Result<GameResult, ClientError> {
    let player_count = tokens.len();
    let lobby_id = http.try_create_lobby(&tokens[0]).await?;

    for token in tokens {
        http.try_join_lobby(token, &lobby_id).await?;
    }

    let mut player_map = HashMap::new();

    for token in tokens {
        let player_id = HttpClient::player_id_from_token(token);
        let socket = ws.connect(token).await?;
        player_map.insert(player_id, socket);
    }

    if player_map.len() != player_count {
        return Err(ClientError(format!(
            "Expected {} WS connections, got {}",
            player_count,
            player_map.len()
        )));
    }

    let session = GameSession::new(
        player_map,
        TurnDelay {
            min_ms: config.min_turn_delay_ms,
            max_ms: config.max_turn_delay_ms,
        },
        TurnDelay {
            min_ms: config.min_bid_delay_ms,
            max_ms: config.max_bid_delay_ms,
        },
    );

    let outcome = session.run_until_end().await?;

    match outcome {
        GameOutcome::GameEnded { lifes } => {
            let mut sorted: Vec<_> = lifes.iter().collect();
            sorted.sort_by_key(|&(_, lifes)| std::cmp::Reverse(*lifes));

            let info = sorted
                .iter()
                .take(3)
                .map(|(id, lifes)| format!("{}:{}", id.as_str(), lifes))
                .collect::<Vec<_>>()
                .join(" ");

            Ok(GameResult {
                player_count,
                outcome: "OK".into(),
                info,
            })
        }
    }
}

fn log_error(msg: &str) {
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
    let line = format!("{ts} {msg}\n");

    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("loadtester_errors.log")
    {
        let _ = file.write_all(line.as_bytes());
    }

    eprintln!("ERROR {msg}");
}
