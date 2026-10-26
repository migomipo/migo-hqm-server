//! Reports the server's state to systemd with the `sd_notify` protocol: readiness, a status line
//! for `systemctl status`, watchdog keep-alives and shutdown. Everything here does nothing when the
//! server was not started by a systemd unit with `Type=notify`, and on systems other than Unix.

use std::time::{Duration, Instant};

/// How often the status line and watchdog keep-alive are sent, unless the watchdog needs them
/// more often.
const STATUS_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) struct Notifier {
    socket: Option<imp::NotifySocket>,
    interval: Duration,
    last_sent: Option<Instant>,
}

impl Notifier {
    pub(crate) fn from_env() -> Self {
        let socket = imp::NotifySocket::from_env();
        let interval = watchdog_interval().map_or(STATUS_INTERVAL, |watchdog| {
            (watchdog / 2).min(STATUS_INTERVAL)
        });
        Self {
            socket,
            interval,
            last_sent: None,
        }
    }

    fn send(&self, state: &str) {
        if let Some(socket) = &self.socket {
            socket.send(state);
        }
    }

    /// Tells systemd that the server has started.
    pub(crate) fn ready(&self) {
        self.send("READY=1");
    }

    /// Tells systemd that the server is shutting down.
    pub(crate) fn stopping(&self) {
        self.send("STOPPING=1");
    }

    /// Called on every tick. Sends the status line made by `status`, and a watchdog keep-alive,
    /// at most once per interval. Since the keep-alives come from the game loop, systemd restarts
    /// the server if the game loop stops ticking.
    pub(crate) fn tick(&mut self, status: impl FnOnce() -> String) {
        if self.socket.is_none() {
            return;
        }
        let now = Instant::now();
        if self
            .last_sent
            .is_some_and(|last| now - last < self.interval)
        {
            return;
        }
        self.last_sent = Some(now);
        self.send(&format!("WATCHDOG=1\nSTATUS={}", status()));
    }
}

/// The watchdog timeout from `WatchdogSec=`, if it is set for this process.
fn watchdog_interval() -> Option<Duration> {
    let usec: u64 = std::env::var("WATCHDOG_USEC").ok()?.parse().ok()?;
    if let Ok(pid) = std::env::var("WATCHDOG_PID")
        && pid.parse::<u32>().ok() != Some(std::process::id())
    {
        return None;
    }
    (usec > 0).then(|| Duration::from_micros(usec))
}

#[cfg(unix)]
mod imp {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::UnixDatagram;
    use tracing::warn;

    pub(super) struct NotifySocket {
        socket: UnixDatagram,
        addr: std::os::unix::net::SocketAddr,
    }

    impl NotifySocket {
        pub(super) fn from_env() -> Option<Self> {
            Self::new(&std::env::var_os("NOTIFY_SOCKET")?)
        }

        pub(super) fn new(path: &OsStr) -> Option<Self> {
            let addr = match socket_addr(path.as_encoded_bytes()) {
                Ok(addr) => addr,
                Err(e) => {
                    warn!("Could not use NOTIFY_SOCKET {path:?}: {e}");
                    return None;
                }
            };
            match UnixDatagram::unbound() {
                Ok(socket) => Some(Self { socket, addr }),
                Err(e) => {
                    warn!("Could not create a socket for systemd notifications: {e}");
                    None
                }
            }
        }

        pub(super) fn send(&self, state: &str) {
            // A lost notification is only a missed status update; the next one follows shortly.
            let _ = self.socket.send_to_addr(state.as_bytes(), &self.addr);
        }
    }

    fn socket_addr(path: &[u8]) -> std::io::Result<std::os::unix::net::SocketAddr> {
        // A leading @ means a socket in Linux's abstract namespace.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if let Some(name) = path.strip_prefix(b"@") {
            #[cfg(target_os = "android")]
            use std::os::android::net::SocketAddrExt;
            #[cfg(target_os = "linux")]
            use std::os::linux::net::SocketAddrExt;
            return std::os::unix::net::SocketAddr::from_abstract_name(name);
        }
        std::os::unix::net::SocketAddr::from_pathname(OsStr::from_bytes(path))
    }
}

#[cfg(not(unix))]
mod imp {
    pub(super) struct NotifySocket;

    impl NotifySocket {
        pub(super) fn from_env() -> Option<Self> {
            None
        }

        pub(super) fn send(&self, _state: &str) {}
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::net::UnixDatagram;

    #[test]
    fn sends_readiness_status_and_stopping() {
        let dir = std::env::temp_dir().join(format!("hqm-notify-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notify.sock");
        let _ = std::fs::remove_file(&path);
        let listener = UnixDatagram::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();

        let mut notifier = Notifier {
            socket: imp::NotifySocket::new(path.as_os_str()),
            interval: Duration::from_secs(60),
            last_sent: None,
        };
        notifier.ready();
        notifier.tick(|| "2/10 players".to_owned());
        // Within the interval, so nothing is sent.
        notifier.tick(|| panic!("status made within the interval"));
        notifier.stopping();

        let mut received = vec![];
        let mut buf = [0; 256];
        while let Ok(len) = listener.recv(&mut buf) {
            received.push(String::from_utf8_lossy(&buf[..len]).into_owned());
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            received,
            [
                "READY=1",
                "WATCHDOG=1
STATUS=2/10 players",
                "STOPPING=1"
            ]
        );
    }
}
