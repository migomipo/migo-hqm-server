use crate::game::PhysicsConfiguration;
use crate::gamemode::GameMode;
use crate::protocol::{HQMClientToServerMessage, HQMMessageCodec};
use crate::server::HQMServer;
use async_stream::stream;
use bytes::BytesMut;
use futures::StreamExt;
use itertools::Itertools;
use notify_debouncer_full::notify::{RecommendedWatcher, RecursiveMode, Watcher};
use notify_debouncer_full::{
    new_debouncer, DebounceEventHandler, DebounceEventResult, Debouncer, FileIdMap,
};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Handle;
use tokio::time::MissedTickBehavior;
use tracing::info;

mod admin_commands;

pub mod gamemode;

pub mod game;
pub mod physics;
mod protocol;
mod server;

/// Starts an HQM server. This method will not return until the server has terminated.
pub async fn run_server<B: GameMode>(
    port: u16,
    public: Option<&str>,
    config: ServerConfiguration,
    physics_config: PhysicsConfiguration,
    ban: BanCheck,
    mut behaviour: B,
) -> std::io::Result<()> {
    let initial_values = behaviour.get_initial_game_values();

    let reqwest_client = reqwest::Client::new();

    let mut server = HQMServer::new(initial_values, config, physics_config, ban);
    info!("Server started");

    behaviour.init((&mut server).into());

    // Set up timers
    let mut tick_timer = tokio::time::interval(Duration::from_millis(10));
    tick_timer.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let socket = Arc::new(tokio::net::UdpSocket::bind(&addr).await?);
    info!(
        "Server listening at address {:?}",
        socket.local_addr().unwrap()
    );

    async fn get_http_response(
        client: &reqwest::Client,
        address: &str,
    ) -> Result<SocketAddr, Box<dyn Error + Send + Sync>> {
        let response = client.get(address).send().await?.text().await?;

        let split = response.split_ascii_whitespace().collect::<Vec<&str>>();

        let addr = split.get(1).unwrap_or(&"").parse::<IpAddr>()?;
        let port = split.get(2).unwrap_or(&"").parse::<u16>()?;
        Ok(SocketAddr::new(addr, port))
    }

    if let Some(public) = public {
        let socket = socket.clone();
        let reqwest_client = reqwest_client.clone();
        let address = public.to_string();
        tokio::spawn(async move {
            loop {
                let master_server = get_http_response(&reqwest_client, &address).await;
                match master_server {
                    Ok(addr) => {
                        for _ in 0..60 {
                            let msg = b"Hock\x20";
                            let res = socket.send_to(msg, addr).await;
                            if res.is_err() {
                                break;
                            }
                            tokio::time::sleep(Duration::from_secs(10)).await;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(e);
                        tokio::time::sleep(Duration::from_secs(15)).await;
                    }
                }
            }
        });
    }
    enum Msg {
        Time,
        Message(SocketAddr, HQMClientToServerMessage),
    }

    let timeout_stream = tokio_stream::wrappers::IntervalStream::new(tick_timer).map(|_| Msg::Time);
    let packet_stream = {
        let socket = socket.clone();
        stream! {
            let mut buf = BytesMut::with_capacity(512);
            let codec = HQMMessageCodec;
            loop {
                buf.clear();

                match socket.recv_buf_from(&mut buf).await {
                    Ok((_, addr)) => {
                        if let Ok(data) = codec.parse_message(&buf) {
                            yield Msg::Message(addr, data)
                        }
                    }
                    Err(_) => {}
                }
            }
        }
    };
    tokio::pin!(packet_stream);

    let mut stream = futures::stream_select!(timeout_stream, packet_stream);
    let mut write_buf = BytesMut::with_capacity(4096);
    while let Some(msg) = stream.next().await {
        match msg {
            Msg::Time => server.tick(&socket, &mut behaviour, &mut write_buf).await,
            Msg::Message(addr, data) => {
                server
                    .handle_message(addr, &socket, data, &mut behaviour, &mut write_buf)
                    .await
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub enum ReplaySaving {
    File,
    Endpoint { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum ReplayEnabled {
    Off,
    On,
    Standby,
}

#[derive(Debug, Clone)]
pub struct ServerConfiguration {
    pub welcome: Vec<String>,
    pub password: Option<String>,
    pub player_max: usize,

    pub replays_enabled: ReplayEnabled,
    pub replay_saving: ReplaySaving,
    pub server_name: String,
    pub server_service: Option<String>,
}

pub enum BanCheck {
    InMemory {
        file: Option<PathBuf>,
        ban_list: Arc<Mutex<HashSet<IpAddr>>>,
        watcher: Option<Debouncer<RecommendedWatcher, FileIdMap>>,
    },
}

impl BanCheck {
    pub fn new_mem() -> Self {
        Self::InMemory {
            file: None,
            ban_list: Arc::new(Default::default()),
            watcher: None,
        }
    }

    pub async fn new_file(path: PathBuf) -> Self {
        let ban_list = Arc::new(Mutex::new(read_ban_file(&path).await.unwrap_or_else(|e| {
            tracing::error!("Ban file error: {}", e);
            HashSet::new()
        })));
        let handle = Handle::current();

        struct BanFileEventHandler {
            path: PathBuf,
            ban_list: Arc<Mutex<HashSet<IpAddr>>>,
            handle: Handle,
        }

        impl DebounceEventHandler for BanFileEventHandler {
            fn handle_event(&mut self, event: DebounceEventResult) {
                if let Ok(_) = event {
                    let ban_list = self.ban_list.clone();
                    let path = self.path.clone();
                    self.handle.spawn(async move {
                        if let Ok(res) = read_ban_file(&path).await {
                            {
                                let mut ban_list = ban_list.lock();
                                *ban_list = res;
                            }
                        }
                    });
                }
            }
        }
        let watcher = new_debouncer(
            Duration::from_secs(1),
            None,
            BanFileEventHandler {
                path: path.clone(),
                ban_list: ban_list.clone(),
                handle,
            },
        )
        .and_then(|mut watcher| {
            watcher
                .watcher()
                .watch(&path, RecursiveMode::NonRecursive)?;
            Ok(watcher)
        });

        Self::InMemory {
            ban_list,
            file: Some(path),
            watcher: watcher.ok(),
        }
    }

    pub(crate) fn insert(&mut self, ip: IpAddr) {
        match self {
            BanCheck::InMemory { ban_list, file, .. } => {
                let mut ban_list = ban_list.lock();
                ban_list.insert(ip);
                if let Some(file) = file {
                    save_ban_to_file(&ban_list, file);
                }
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        match self {
            BanCheck::InMemory { ban_list, file, .. } => {
                let mut ban_list = ban_list.lock();
                ban_list.clear();
                if let Some(file) = file {
                    save_ban_to_file(&ban_list, file);
                }
            }
        }
    }

    pub(crate) fn is_banned(&self, ip: IpAddr) -> bool {
        match self {
            BanCheck::InMemory { ban_list, .. } => {
                let ban_list = ban_list.lock();
                ban_list.contains(&ip)
            }
        }
    }
}

async fn read_ban_file(path: &Path) -> Result<HashSet<IpAddr>, tokio::io::Error> {
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .await?;
    let mut s = String::new();
    f.read_to_string(&mut s).await?;
    let mut res = HashSet::new();
    for line in s.lines() {
        if let Ok(ip) = line.parse::<IpAddr>() {
            res.insert(ip);
        }
    }
    Ok(res)
}

fn save_ban_to_file(bans: &HashSet<IpAddr>, path: &Path) {
    let path = path.to_path_buf();
    let s = bans.iter().map(|x| format!("{}\n", x)).join("");
    tokio::spawn(async move {
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .await?;
        f.write_all(s.as_bytes()).await?;
        f.flush().await?;
        Ok::<_, tokio::io::Error>(())
    });
}
