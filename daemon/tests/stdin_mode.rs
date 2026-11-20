//! Runs the real daemon binary in `--stdin` test mode and follows it with the
//! same client the applets use. Needs no access to /dev/input.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use lock_keys_protocol::{client, LockState};
use tokio::sync::mpsc;
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(10);

async fn next(rx: &mut mpsc::UnboundedReceiver<Option<LockState>>) -> Option<LockState> {
    timeout(WAIT, rx.recv()).await.unwrap().unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn applet_client_follows_daemon() {
    let dir = std::env::temp_dir().join(format!("lock-keys-stdin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("daemon.sock");
    let _ = std::fs::remove_file(&path);

    let (tx, mut rx) = mpsc::unbounded_channel();
    let watch_path = path.clone();
    let watcher = tokio::spawn(async move {
        client::watch(&watch_path, |state| {
            let tx = tx.clone();
            async move {
                let _ = tx.send(state);
            }
        })
        .await;
    });
    // No daemon yet.
    assert_eq!(next(&mut rx).await, None);

    let mut daemon = Command::new(env!("CARGO_BIN_EXE_lock-keys-daemon"))
        .arg("--stdin")
        .env("LOCK_KEYS_SOCKET", &path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = daemon.stdin.take().unwrap();

    // The client retries once a second and gets the initial state.
    assert_eq!(next(&mut rx).await, Some(LockState::default()));

    writeln!(stdin, "caps_lock=on num_lock=off").unwrap();
    assert_eq!(
        next(&mut rx).await,
        Some(LockState {
            caps_lock: true,
            num_lock: false,
        })
    );

    writeln!(stdin, "not a state").unwrap();
    writeln!(stdin, "caps_lock=on num_lock=on").unwrap();
    assert_eq!(
        next(&mut rx).await,
        Some(LockState {
            caps_lock: true,
            num_lock: true,
        })
    );

    // Closing stdin stops the daemon; the client reports it as gone.
    drop(stdin);
    assert!(daemon.wait().unwrap().success());
    assert_eq!(next(&mut rx).await, None);
    assert!(!path.exists());

    watcher.abort();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rejects_unknown_arguments() {
    let status = Command::new(env!("CARGO_BIN_EXE_lock-keys-daemon"))
        .arg("--bogus")
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(2));
}
