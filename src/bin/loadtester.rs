use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use rand::{RngExt, rng};
use ratatui::{prelude::*, widgets::*};
use tokio::sync::mpsc;

use oh_hell::client::{GameOutcome, GameSession, HttpClient, TurnDelay, WsClient};

#[derive(serde::Deserialize, Clone)]
struct Config {
    host: String,
    min_players: usize,
    max_players: usize,
    min_turn_delay_ms: u64,
    max_turn_delay_ms: u64,
    min_bid_delay_ms: u64,
    max_bid_delay_ms: u64,
    #[serde(default = "default_initial_games")]
    initial_games: usize,
}

fn default_initial_games() -> usize {
    1
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
            initial_games: 1,
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
    results: Mutex<Vec<GameResult>>,
    shutdown: AtomicBool,
}

impl SharedState {
    fn new(initial: usize) -> Self {
        Self {
            desired_games: AtomicUsize::new(initial),
            running_games: AtomicUsize::new(0),
            finished_games: AtomicUsize::new(0),
            error_count: AtomicUsize::new(0),
            results: Mutex::new(Vec::new()),
            shutdown: AtomicBool::new(false),
        }
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let config = load_config();

    let ws_url = config
        .host
        .replace("http://", "ws://")
        .replace("https://", "wss://");

    let state = Arc::new(SharedState::new(config.initial_games));

    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<RunnerCmd>();

    let runner_state = state.clone();
    let runner_config = config.clone();
    let runner_http = HttpClient::new(config.host.clone());
    let runner_ws = WsClient::new(ws_url);
    let runner_tx = cmd_tx.clone();

    tokio::spawn(async move {
        run_game_manager(
            runner_state,
            runner_config,
            runner_http,
            runner_ws,
            runner_tx,
            &mut cmd_rx,
        )
        .await;
    });

    run_tui(state, config, cmd_tx).await
}

fn load_config() -> Config {
    let config_path = "config.json";
    match std::fs::read_to_string(config_path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            eprintln!("Invalid config.json: {e}, using defaults");
            Config::default()
        }),
        Err(_) => {
            eprintln!("No config.json found at {config_path}, using defaults");
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
    cmd_rx: &mut mpsc::UnboundedReceiver<RunnerCmd>,
) {
    loop {
        let desired = state.desired_games.load(Ordering::Relaxed);
        let running = state.running_games.load(Ordering::Relaxed);

        if desired > running {
            for _ in 0..(desired - running) {
                let s = state.clone();
                let c = config.clone();
                let h = http.clone();
                let w = ws.clone();
                let tx = cmd_tx.clone();

                s.running_games.fetch_add(1, Ordering::Relaxed);

                tokio::spawn(async move {
                    let player_count = {
                        let mut rng = rng();
                        rng.random_range(c.min_players..=c.max_players)
                    };

                    match run_single_game(player_count, h, w, &c).await {
                        Ok(result) => {
                            let _ = tx.send(RunnerCmd::GameFinished(result));
                        }
                        Err(e) => {
                            s.error_count.fetch_add(1, Ordering::Relaxed);
                            let _ = tx.send(RunnerCmd::GameFinished(GameResult {
                                player_count,
                                outcome: "ERROR".into(),
                                info: e,
                            }));
                        }
                    }

                    s.running_games.fetch_sub(1, Ordering::Relaxed);
                });
            }
        }

        if state.shutdown.load(Ordering::Relaxed) {
            return;
        }

        match tokio::time::timeout(Duration::from_millis(250), cmd_rx.recv()).await {
            Ok(Some(RunnerCmd::SetDesired)) | Ok(Some(RunnerCmd::Shutdown)) | Ok(None) => {}
            Ok(Some(RunnerCmd::GameFinished(result))) => {
                state.finished_games.fetch_add(1, Ordering::Relaxed);
                state.results.lock().unwrap().push(result);
            }
            Err(_) => {}
        }
    }
}

async fn run_single_game(
    player_count: usize,
    http: HttpClient,
    ws: WsClient,
    config: &Config,
) -> Result<GameResult, String> {
    let mut tokens: Vec<String> = Vec::with_capacity(player_count);

    for i in 0..player_count {
        let nickname = format!("Bot{}", i + 1);
        let token = http.signup(&nickname).await;
        tokens.push(token);
    }

    let lobby_id = http.create_lobby(&tokens[0]).await;

    for token in &tokens {
        http.join_lobby(token, &lobby_id).await;
    }

    let mut player_map = HashMap::new();

    for token in &tokens {
        let player_id = HttpClient::player_id_from_token(token);
        let socket = ws.connect(token).await;
        player_map.insert(player_id, socket);
    }

    if player_map.len() != player_count {
        return Err(format!(
            "Expected {} players in websocket map, got {}",
            player_count,
            player_map.len()
        ));
    }

    let turn_delay = TurnDelay {
        min_ms: config.min_turn_delay_ms,
        max_ms: config.max_turn_delay_ms,
    };

    let bid_delay = TurnDelay {
        min_ms: config.min_bid_delay_ms,
        max_ms: config.max_bid_delay_ms,
    };

    let session = GameSession::new(player_map, http, ws, turn_delay, bid_delay);
    let outcome = session.run_until_end().await;

    match outcome {
        GameOutcome::GameEnded { lifes } => {
            let mut sorted: Vec<_> = lifes.iter().collect();
            sorted.sort_by_key(|&(_, lifes)| std::cmp::Reverse(*lifes));

            let info = sorted
                .iter()
                .map(|(id, lifes)| format!("{}: {lifes}", id.as_str()))
                .collect::<Vec<_>>()
                .join(", ");

            Ok(GameResult {
                player_count,
                outcome: "ENDED".into(),
                info,
            })
        }
        GameOutcome::SetEnded { lifes: _ } => Ok(GameResult {
            player_count,
            outcome: "SET_ENDED".into(),
            info: "Unexpected set end".into(),
        }),
        GameOutcome::Error(e) => Err(e),
    }
}

async fn run_tui(
    state: Arc<SharedState>,
    config: Config,
    cmd_tx: mpsc::UnboundedSender<RunnerCmd>,
) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = run_app(&mut terminal, state, config, cmd_tx).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    res
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: Arc<SharedState>,
    _config: Config,
    cmd_tx: mpsc::UnboundedSender<RunnerCmd>,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| {
            render(f, &state, &_config);
        })?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Release {
                continue;
            }

            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
            let multiplier: usize = if shift { 10 } else { 1 };

            match key.code {
                KeyCode::Char('q') | KeyCode::Char('Q') => {
                    state.shutdown.store(true, Ordering::Relaxed);
                    let _ = cmd_tx.send(RunnerCmd::Shutdown);

                    return Ok(());
                }
                KeyCode::Up => {
                    let current = state.desired_games.load(Ordering::Relaxed);
                    let new = current.saturating_add(multiplier);
                    state.desired_games.store(new, Ordering::Relaxed);
                    let _ = cmd_tx.send(RunnerCmd::SetDesired);
                }
                KeyCode::Down => {
                    let current = state.desired_games.load(Ordering::Relaxed);
                    let new = current.saturating_sub(multiplier);
                    state.desired_games.store(new, Ordering::Relaxed);
                    let _ = cmd_tx.send(RunnerCmd::SetDesired);
                }
                _ => {}
            }
        }
    }
}

fn render(frame: &mut Frame, state: &SharedState, _config: &Config) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let desired = state.desired_games.load(Ordering::Relaxed);
    let running = state.running_games.load(Ordering::Relaxed);
    let finished = state.finished_games.load(Ordering::Relaxed);
    let errors = state.error_count.load(Ordering::Relaxed);

    let status = Paragraph::new(format!(
        "Desired: {} | Running: {} | Finished: {} | Errors: {}",
        desired, running, finished, errors,
    ))
    .style(Style::default().fg(Color::Cyan))
    .block(Block::default().borders(Borders::ALL).title("Status"));
    frame.render_widget(status, chunks[0]);

    let controls = Paragraph::new("Up/Down: +/-1 game | Shift+Up/Down: +/-10 games | Q: Quit")
        .style(Style::default().fg(Color::Yellow))
        .block(Block::default().borders(Borders::ALL).title("Controls"));
    frame.render_widget(controls, chunks[1]);

    let results = state.results.lock().unwrap();
    let result_lines: Vec<Line> = results
        .iter()
        .rev()
        .take(30)
        .map(|r| Line::from(format!("[{}P] {} | {}", r.player_count, r.outcome, r.info)))
        .collect();

    let history = Paragraph::new(Text::from(result_lines))
        .block(Block::default().borders(Borders::ALL).title("Recent Games"))
        .scroll((0, 0));
    frame.render_widget(history, chunks[2]);

    let help = Paragraph::new("Load Tester for Oh Hell API")
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(help, chunks[3]);
}
