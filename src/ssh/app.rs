use base64::Engine;
use ratatui::{
    Terminal,
    layout::Rect,
    style::{Color, Style},
    widgets::*,
};
use russh::{Channel, ChannelId, keys, server::*};
use std::{
    collections::HashMap,
    io::Error,
    net::Ipv6Addr,
    sync::atomic::{AtomicUsize, Ordering},
};
use std::{io, net::SocketAddr, sync::Arc};
use tokio::{sync::Mutex, time::Duration};

use crate::{
    AppSettings,
    ssh::{SshError, backend::SshBackend},
};

type SshTerminal = Terminal<SshBackend>;

#[derive(Clone)]
pub struct TerminalHandle {
    handle: Handle,
    sink: Vec<u8>,
    channel_id: ChannelId,
}

impl std::io::Write for TerminalHandle {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sink.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let handle = self.handle.clone();
        let channel_id = self.channel_id;
        let data = self.sink.clone().into();

        let result = futures::executor::block_on(async {
            match handle.data(channel_id, data).await {
                Ok(_) => Ok(()),
                Err(_) => {
                    tracing::error!("Failed to send data");
                    return handle.close(channel_id).await;
                }
            }
        });

        self.sink.clear();
        result.map_err(|_| Error::other("something went wrong"))
    }
}

#[derive(Clone)]
pub struct AppServer {
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

        tokio::spawn(game_loop(app));

        let key = base64::engine::general_purpose::STANDARD
            .decode(&settings.ssh_host_key)
            .expect("valid base64");

        let host_key = keys::PrivateKey::from_openssh(key).expect("valid host key");

        let config = Config {
            inactivity_timeout: Some(Duration::from_secs(30)),
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
        let terminal_handle = TerminalHandle {
            handle: session.handle(),
            sink: Vec::new(),
            channel_id: channel.id(),
        };

        let backend = SshBackend::new(terminal_handle);
        let terminal = Terminal::new(backend)?;

        {
            let mut clients = self.clients.lock().await;
            clients.insert(self.id, terminal);
        }

        Ok(true)
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        key: &russh::keys::PublicKey,
    ) -> Result<Auth, Self::Error> {
        tracing::debug!(
            "user {} connected with key {}",
            user,
            key.fingerprint(keys::HashAlg::Sha256)
        );
        Ok(Auth::Accept)
    }

    async fn channel_close(
        &mut self,
        _channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.clients.lock().await.remove(&self.id);

        Ok(())
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

        tracing::debug!(
            "client {} on channel {} sent {:?}|{}",
            self.id,
            channel,
            data,
            data[0] as char
        );

        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _: ChannelId,
        width: u32,
        height: u32,
        _: u32,
        _: u32,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        {
            let mut clients = self.clients.lock().await;
            let terminal = clients.get_mut(&self.id).unwrap();
            tracing::debug!("client {} resizing to {:?}", self.id, (height, width));
            let rect = Rect {
                x: 0,
                y: 0,
                width: width as u16,
                height: height as u16,
            };
            terminal.resize(rect)?;
            terminal.backend_mut().resize(width as u16, height as u16);
        }

        Ok(())
    }
}

async fn game_loop(app: AppServer) {
    let mut disconnected = vec![];

    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let counter = app.counter.fetch_add(1, Ordering::Relaxed);

        for (id, terminal) in app.clients.lock().await.iter_mut() {
            let draw_result = terminal.draw(|f| {
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
            });

            if draw_result.is_err() {
                disconnected.push(*id);
            }
        }

        let mut clients = app.clients.lock().await;

        for d in disconnected.drain(..) {
            clients.remove(&d);
        }
    }
}
