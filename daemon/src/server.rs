//! Unix socket that sends the lock state to every connected applet.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use lock_keys_protocol::LockState;
use log::{debug, warn};

struct Shared {
    state: LockState,
    clients: Vec<UnixStream>,
}

pub struct Server {
    path: PathBuf,
    shared: Arc<Mutex<Shared>>,
}

impl Server {
    /// Listens at `path`. Each client gets `initial` (or the latest published
    /// state) as soon as it connects.
    pub fn bind(path: &Path, initial: LockState) -> io::Result<Server> {
        if UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("another daemon is already listening on {}", path.display()),
            ));
        }
        // A socket file left behind by a daemon that was killed.
        match fs::remove_file(path) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }

        let listener = UnixListener::bind(path)?;
        // The daemon runs as root and the applets as the desktop user.
        fs::set_permissions(path, fs::Permissions::from_mode(0o666))?;

        let shared = Arc::new(Mutex::new(Shared {
            state: initial,
            clients: Vec::new(),
        }));

        let accept_shared = Arc::clone(&shared);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(stream) => stream,
                    Err(err) => {
                        warn!("accept failed: {err}");
                        continue;
                    }
                };
                // A client that stops reading is dropped instead of blocking the daemon.
                if stream.set_nonblocking(true).is_err() {
                    continue;
                }
                let mut shared = accept_shared.lock().unwrap();
                if stream.write_all(shared.state.to_line().as_bytes()).is_ok() {
                    debug!("client connected");
                    shared.clients.push(stream);
                }
            }
        });

        Ok(Server {
            path: path.to_path_buf(),
            shared,
        })
    }

    /// Sends `state` to every client if it differs from the last one.
    pub fn publish(&self, state: LockState) {
        let mut shared = self.shared.lock().unwrap();
        if shared.state == state {
            return;
        }
        shared.state = state;
        let line = state.to_line();
        shared
            .clients
            .retain_mut(|client| client.write_all(line.as_bytes()).is_ok());
        debug!("sent {state} to {} client(s)", shared.clients.len());
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::time::Duration;

    /// A socket path in its own temporary directory.
    fn socket(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lock-keys-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir.join("test.sock")
    }

    fn remove_dir(path: &Path) {
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    fn connect(path: &Path) -> BufReader<UnixStream> {
        let stream = UnixStream::connect(path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        BufReader::new(stream)
    }

    fn read_line(client: &mut BufReader<UnixStream>) -> io::Result<String> {
        let mut line = String::new();
        client.read_line(&mut line)?;
        Ok(line)
    }

    const CAPS: LockState = LockState {
        caps_lock: true,
        num_lock: false,
    };
    const BOTH: LockState = LockState {
        caps_lock: true,
        num_lock: true,
    };

    #[test]
    fn sends_current_state_then_changes() {
        let path = socket("changes");
        let server = Server::bind(&path, LockState::default()).unwrap();

        let mut first = connect(&path);
        assert_eq!(
            read_line(&mut first).unwrap(),
            "caps_lock=off num_lock=off\n"
        );

        server.publish(CAPS);
        assert_eq!(
            read_line(&mut first).unwrap(),
            "caps_lock=on num_lock=off\n"
        );

        // A late client starts from the latest state.
        let mut second = connect(&path);
        assert_eq!(
            read_line(&mut second).unwrap(),
            "caps_lock=on num_lock=off\n"
        );

        // Unchanged states aren't sent again.
        server.publish(CAPS);
        assert!(read_line(&mut first).is_err());

        server.publish(BOTH);
        assert_eq!(read_line(&mut first).unwrap(), "caps_lock=on num_lock=on\n");
        assert_eq!(
            read_line(&mut second).unwrap(),
            "caps_lock=on num_lock=on\n"
        );

        drop(server);
        assert!(!path.exists());
        remove_dir(&path);
    }

    #[test]
    fn drops_closed_clients() {
        let path = socket("closed");
        let server = Server::bind(&path, LockState::default()).unwrap();
        let mut client = connect(&path);
        read_line(&mut client).unwrap();
        drop(client);

        server.publish(CAPS);
        server.publish(BOTH);
        assert!(server.shared.lock().unwrap().clients.is_empty());
        drop(server);
        remove_dir(&path);
    }

    #[test]
    fn replaces_stale_socket_but_not_a_live_one() {
        let path = socket("stale");
        drop(UnixListener::bind(&path).unwrap());
        assert!(path.exists());

        let server = Server::bind(&path, LockState::default()).unwrap();
        let err = Server::bind(&path, LockState::default()).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
        drop(server);
        remove_dir(&path);
    }
}
