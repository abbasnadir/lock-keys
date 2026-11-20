//! Async client for the applets.

use std::future::Future;
use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixStream;

use crate::LockState;

/// How long to wait before reconnecting after the daemon went away.
pub const RETRY_DELAY: Duration = Duration::from_secs(1);

/// Follows the daemon listening at `path` and never returns.
///
/// `on_change` gets every new state the daemon sends, and `None` when the
/// daemon can't be reached or the connection drops. Repeats of the same value
/// are skipped. After a failure it tries again every [`RETRY_DELAY`].
pub async fn watch<F, Fut>(path: &Path, mut on_change: F)
where
    F: FnMut(Option<LockState>) -> Fut,
    Fut: Future<Output = ()>,
{
    let mut last = None;

    loop {
        if let Ok(stream) = UnixStream::connect(path).await {
            let mut lines = BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(state) = line.parse() {
                    report(&mut on_change, &mut last, Some(state)).await;
                }
            }
        }

        report(&mut on_change, &mut last, None).await;
        tokio::time::sleep(RETRY_DELAY).await;
    }
}

async fn report<F, Fut>(
    on_change: &mut F,
    last: &mut Option<Option<LockState>>,
    state: Option<LockState>,
) where
    F: FnMut(Option<LockState>) -> Fut,
    Fut: Future<Output = ()>,
{
    if *last != Some(state) {
        *last = Some(state);
        on_change(state).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::UnixListener;
    use tokio::sync::mpsc;
    use tokio::time::timeout;

    const WAIT: Duration = Duration::from_secs(5);

    #[tokio::test(flavor = "current_thread")]
    async fn reports_states_and_missing_daemon() {
        let dir = std::env::temp_dir().join(format!("lock-keys-client-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.sock");
        let _ = std::fs::remove_file(&path);

        let (tx, mut rx) = mpsc::unbounded_channel();
        let watch_path = path.clone();
        let watcher = tokio::spawn(async move {
            watch(&watch_path, |state| {
                let tx = tx.clone();
                async move {
                    let _ = tx.send(state);
                }
            })
            .await;
        });

        // Nothing listens yet.
        assert_eq!(timeout(WAIT, rx.recv()).await.unwrap(), Some(None));

        let listener = UnixListener::bind(&path).unwrap();
        let (mut daemon, _) = timeout(WAIT, listener.accept()).await.unwrap().unwrap();
        let on = LockState {
            caps_lock: true,
            num_lock: false,
        };
        daemon
            .write_all(b"caps_lock=on num_lock=off\ngarbage\ncaps_lock=on num_lock=off\n")
            .await
            .unwrap();
        daemon
            .write_all(b"caps_lock=on num_lock=on\n")
            .await
            .unwrap();
        assert_eq!(timeout(WAIT, rx.recv()).await.unwrap(), Some(Some(on)));
        // The garbage line and the repeated state are skipped.
        assert_eq!(
            timeout(WAIT, rx.recv()).await.unwrap(),
            Some(Some(LockState {
                caps_lock: true,
                num_lock: true,
            }))
        );

        // The daemon goes away.
        drop(daemon);
        drop(listener);
        assert_eq!(timeout(WAIT, rx.recv()).await.unwrap(), Some(None));

        watcher.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
