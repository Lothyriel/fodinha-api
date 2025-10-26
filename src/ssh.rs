use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::Rect,
    style::{Color, Style},
    widgets::*,
};
use russh::{Channel, ChannelId, keys, server::*};
use std::{
    collections::HashMap,
    net::Ipv6Addr,
    sync::atomic::{AtomicUsize, Ordering},
};
use std::{net::SocketAddr, sync::Arc};
use tokio::{sync::Mutex, time::Duration};

use crate::{AppSettings, Manager};

type SshTerminal = Terminal<CrosstermBackend<TerminalHandle>>;

#[derive(Clone)]
struct TerminalHandle {
    handle: Handle,
    sink: Vec<u8>,
    channel_id: ChannelId,
}

impl std::io::Write for TerminalHandle {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.sink.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let handle = self.handle.clone();
        let channel_id = self.channel_id;
        let data = self.sink.clone().into();
        futures::executor::block_on(async move {
            let result = handle.data(channel_id, data).await;
            if result.is_err() {
                tracing::error!("Failed to send data: {result:?}");
            }
        });

        self.sink.clear();
        Ok(())
    }
}

#[derive(Clone)]
struct AppServer {
    clients: Arc<Mutex<HashMap<usize, SshTerminal>>>,
    counter: Arc<AtomicUsize>,
    id: usize,
}

impl AppServer {
    pub fn new() -> Self {
        Self {
            clients: Arc::new(Mutex::new(HashMap::new())),
            counter: Arc::new(AtomicUsize::new(0)),
            id: 0,
        }
    }

    pub async fn run(&mut self, settings: &AppSettings) -> std::io::Result<()> {
        let app = self.clone();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;

                for (_, terminal) in app.clients.lock().await.iter_mut() {
                    let counter = app.counter.fetch_add(1, Ordering::Relaxed);

                    terminal
                        .draw(|f| {
                            let size = f.area();
                            f.render_widget(Clear, size);
                            let style = match counter % 3 {
                                0 => Style::default().fg(Color::Red),
                                1 => Style::default().fg(Color::Green),
                                _ => Style::default().fg(Color::Blue),
                            };
                            let paragraph = Paragraph::new(format!("Counter: {counter}"))
                                .alignment(ratatui::layout::Alignment::Center)
                                .style(style);
                            let block = Block::default()
                                .title("Press 'c' to reset the counter!")
                                .borders(Borders::ALL);
                            f.render_widget(paragraph.block(block), size);
                        })
                        .unwrap();
                }
            }
        });

        let host_key =
            keys::PrivateKey::from_openssh(&settings.ssh_host_key).expect("valid host key");

        let config = Config {
            inactivity_timeout: Some(Duration::from_secs(3600)),
            auth_rejection_time: Duration::from_secs(3),
            auth_rejection_time_initial: Some(Duration::from_secs(0)),
            keys: vec![host_key],
            ..Default::default()
        };

        let addr = (Ipv6Addr::UNSPECIFIED, settings.ssh_port);

        tracing::info!("Listening on {:?}", addr);

        self.run_on_address(Arc::new(config), addr).await?;

        Ok(())
    }
}

impl Server for AppServer {
    type Handler = Self;
    fn new_client(&mut self, _: Option<SocketAddr>) -> Self {
        let s = self.clone();
        self.id += 1;
        s
    }
}

impl Handler for AppServer {
    type Error = SshError;

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        {
            let mut clients = self.clients.lock().await;
            let terminal_handle = TerminalHandle {
                handle: session.handle(),
                sink: Vec::new(),
                channel_id: channel.id(),
            };

            let backend = CrosstermBackend::new(terminal_handle.clone());
            let terminal = Terminal::new(backend)?;

            clients.insert(self.id, terminal);
        }

        Ok(true)
    }

    async fn auth_publickey(
        &mut self,
        _: &str,
        _: &russh::keys::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        match data {
            [3] => {
                self.clients.lock().await.remove(&self.id);
                session.close(channel)?;
            }
            b"c" => {
                self.counter.store(0, Ordering::Relaxed);
            }
            _ => {}
        }

        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _: ChannelId,
        col_width: u32,
        row_height: u32,
        _: u32,
        _: u32,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        {
            let mut clients = self.clients.lock().await;
            let terminal = clients.get_mut(&self.id).unwrap();
            let rect = Rect {
                x: 0,
                y: 0,
                width: col_width as u16,
                height: row_height as u16,
            };
            terminal.resize(rect)?;
        }

        Ok(())
    }
}

pub async fn start(_manager: Manager, settings: &AppSettings) {
    AppServer::new()
        .run(settings)
        .await
        .expect("Failed running server");
}

#[derive(thiserror::Error, Debug)]
pub enum SshError {
    #[error("{0}")]
    IO(#[from] std::io::Error),
    #[error("{0}")]
    Russh(#[from] russh::Error),
}
