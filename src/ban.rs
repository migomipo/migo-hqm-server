use cached::{Cached, TimedCache};
use itertools::Itertools;
use notify_debouncer_full::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{
    new_debouncer, DebounceEventHandler, DebounceEventResult, Debouncer, RecommendedCache,
};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::future::Future;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::runtime::Handle;

#[derive(Eq, PartialEq, Copy, Clone, Debug)]
pub enum BanCheckResponse {
    Allowed,
    Banned,
    Pending,
}

pub trait BanCheck {
    fn check_ip_banned(&mut self, ip_addr: IpAddr) -> BanCheckResponse;
    fn ban_ip(&mut self, ip_addr: IpAddr);

    fn clear_all_bans(&mut self);
}

impl<T> BanCheck for Box<T>
where
    T: BanCheck,
{
    fn check_ip_banned(&mut self, ip_addr: IpAddr) -> BanCheckResponse {
        self.as_mut().check_ip_banned(ip_addr)
    }

    fn ban_ip(&mut self, ip_addr: IpAddr) {
        self.as_mut().ban_ip(ip_addr)
    }

    fn clear_all_bans(&mut self) {
        self.as_mut().clear_all_bans();
    }
}

pub struct InMemoryBanCheck {
    bans: HashSet<IpAddr>,
}

impl InMemoryBanCheck {
    pub fn new() -> Self {
        Self {
            bans: HashSet::new(),
        }
    }
}

impl BanCheck for InMemoryBanCheck {
    fn check_ip_banned(&mut self, ip_addr: IpAddr) -> BanCheckResponse {
        if self.bans.contains(&ip_addr) {
            BanCheckResponse::Banned
        } else {
            BanCheckResponse::Allowed
        }
    }

    fn ban_ip(&mut self, ip_addr: IpAddr) {
        self.bans.insert(ip_addr);
    }

    fn clear_all_bans(&mut self) {
        self.bans.clear();
    }
}

pub struct FileBanCheck {
    file: PathBuf,
    ban_list: Arc<Mutex<HashSet<IpAddr>>>,
    watcher: Debouncer<RecommendedWatcher, RecommendedCache>,
}

impl FileBanCheck {
    pub async fn new(path: PathBuf) -> Result<Self, anyhow::Error> {
        let ban_list = Arc::new(Mutex::new(read_ban_file(&path).await?));
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
        let mut watcher = new_debouncer(
            Duration::from_secs(1),
            None,
            BanFileEventHandler {
                path: path.clone(),
                ban_list: ban_list.clone(),
                handle,
            },
        )?;
        watcher.watch(&path, RecursiveMode::NonRecursive)?;
        Ok(Self {
            ban_list,
            file: path,
            watcher,
        })
    }
}

impl BanCheck for FileBanCheck {
    fn check_ip_banned(&mut self, ip_addr: IpAddr) -> BanCheckResponse {
        let ban_list = self.ban_list.lock();
        if ban_list.contains(&ip_addr) {
            BanCheckResponse::Banned
        } else {
            BanCheckResponse::Allowed
        }
    }

    fn ban_ip(&mut self, ip_addr: IpAddr) {
        let s = {
            let mut ban_list = self.ban_list.lock();
            ban_list.insert(ip_addr);
            ban_list.iter().map(|x| format!("{}\n", x)).join("")
        };
        let path = self.file.clone();

        tokio::spawn(async move { write_ban_file(&path, &s).await });
    }

    fn clear_all_bans(&mut self) {
        let s = {
            let mut ban_list = self.ban_list.lock();
            ban_list.clear();
            ban_list.iter().map(|x| format!("{}\n", x)).join("")
        };
        let path = self.file.clone();

        tokio::spawn(async move {
            let _ = write_ban_file(&path, &s).await;
        });
    }
}

async fn write_ban_file(path: &Path, s: &str) -> Result<(), tokio::io::Error> {
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .await?;
    f.write_all(s.as_bytes()).await?;
    f.flush().await?;
    Ok(())
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

pub trait ExternalBanCheckRequests {
    fn req_ip_banned(&mut self, ip_addr: IpAddr) -> impl Future<Output = bool> + Send + 'static;
    fn req_ban_ip(&mut self, ip_addr: IpAddr) -> impl Future<Output = ()> + Send + 'static;

    fn req_clear_all_bans(&mut self) -> impl Future<Output = ()> + Send + 'static;
}
pub struct ExternalBanCheck<E> {
    cache: Arc<Mutex<TimedCache<IpAddr, BanCheckResponse>>>,
    req: E,
}

impl<E: ExternalBanCheckRequests> ExternalBanCheck<E> {
    pub fn new(req: E) -> Self {
        Self {
            cache: Arc::new(Mutex::new(TimedCache::with_lifespan(10))),
            req,
        }
    }
}

impl<E: ExternalBanCheckRequests> BanCheck for ExternalBanCheck<E> {
    fn check_ip_banned(&mut self, ip_addr: IpAddr) -> BanCheckResponse {
        {
            let mut handle = self.cache.lock();
            if let Some(res) = handle.cache_get(&ip_addr) {
                return *res;
            } else {
                handle.cache_set(ip_addr, BanCheckResponse::Pending);
            }
        }

        let req = self.req.req_ip_banned(ip_addr);
        let cache = self.cache.clone();
        tokio::spawn(async move {
            let res = req.await;
            let mut handle = cache.lock();
            handle.cache_set(
                ip_addr,
                if res {
                    BanCheckResponse::Banned
                } else {
                    BanCheckResponse::Allowed
                },
            );
        });

        BanCheckResponse::Pending
    }

    fn ban_ip(&mut self, ip_addr: IpAddr) {
        self.cache
            .lock()
            .cache_set(ip_addr, BanCheckResponse::Banned);
        let req = self.req.req_ban_ip(ip_addr);

        tokio::spawn(req);
    }

    fn clear_all_bans(&mut self) {
        self.cache.lock().cache_clear();
        let req = self.req.req_clear_all_bans();

        tokio::spawn(req);
    }
}
