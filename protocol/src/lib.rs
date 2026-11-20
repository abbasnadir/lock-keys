//! The line protocol between `lock-keys-daemon` and the panel applets.
//!
//! The daemon listens on a Unix socket. As soon as a client connects it sends
//! the current state as one line, then one more line every time the state
//! changes:
//!
//! ```text
//! caps_lock=on num_lock=off
//! ```

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

#[cfg(feature = "client")]
pub mod client;

/// Where the daemon listens unless `LOCK_KEYS_SOCKET` says otherwise.
/// systemd creates `/run/lock-keys` through `RuntimeDirectory=` in the unit.
pub const DEFAULT_SOCKET_PATH: &str = "/run/lock-keys/lock-keys.sock";

/// Environment variable that overrides [`DEFAULT_SOCKET_PATH`].
pub const SOCKET_ENV: &str = "LOCK_KEYS_SOCKET";

/// The socket path for this process: `$LOCK_KEYS_SOCKET` if set, else the default.
pub fn socket_path() -> PathBuf {
    socket_path_from(std::env::var_os(SOCKET_ENV))
}

fn socket_path_from(value: Option<OsString>) -> PathBuf {
    match value {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => PathBuf::from(DEFAULT_SOCKET_PATH),
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LockState {
    pub caps_lock: bool,
    pub num_lock: bool,
}

impl LockState {
    /// The state as one protocol line, including the trailing newline.
    pub fn to_line(self) -> String {
        format!("{self}\n")
    }
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

impl fmt::Display for LockState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "caps_lock={} num_lock={}",
            on_off(self.caps_lock),
            on_off(self.num_lock)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// A token without `=`.
    BadToken(String),
    /// A value other than `on` or `off`.
    BadValue(String),
    /// A required key is missing.
    Missing(&'static str),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::BadToken(token) => write!(f, "expected key=value, got {token:?}"),
            ParseError::BadValue(token) => write!(f, "expected on or off in {token:?}"),
            ParseError::Missing(key) => write!(f, "missing {key}"),
        }
    }
}

impl std::error::Error for ParseError {}

impl FromStr for LockState {
    type Err = ParseError;

    fn from_str(line: &str) -> Result<Self, Self::Err> {
        let mut caps_lock = None;
        let mut num_lock = None;

        for token in line.split_whitespace() {
            let (key, value) = token
                .split_once('=')
                .ok_or_else(|| ParseError::BadToken(token.to_string()))?;
            let value = match value {
                "on" => true,
                "off" => false,
                _ => return Err(ParseError::BadValue(token.to_string())),
            };
            match key {
                "caps_lock" => caps_lock = Some(value),
                "num_lock" => num_lock = Some(value),
                // Unknown keys are skipped so the daemon can report more later.
                _ => {}
            }
        }

        Ok(LockState {
            caps_lock: caps_lock.ok_or(ParseError::Missing("caps_lock"))?,
            num_lock: num_lock.ok_or(ParseError::Missing("num_lock"))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_one_line() {
        let state = LockState {
            caps_lock: true,
            num_lock: false,
        };
        assert_eq!(state.to_line(), "caps_lock=on num_lock=off\n");
    }

    #[test]
    fn round_trips_every_state() {
        for caps_lock in [false, true] {
            for num_lock in [false, true] {
                let state = LockState {
                    caps_lock,
                    num_lock,
                };
                assert_eq!(state.to_line().parse(), Ok(state));
            }
        }
    }

    #[test]
    fn accepts_any_order_and_unknown_keys() {
        assert_eq!(
            "num_lock=on scroll_lock=off caps_lock=off".parse(),
            Ok(LockState {
                caps_lock: false,
                num_lock: true,
            })
        );
    }

    #[test]
    fn rejects_bad_lines() {
        assert_eq!(
            "".parse::<LockState>(),
            Err(ParseError::Missing("caps_lock"))
        );
        assert_eq!(
            "caps_lock=on".parse::<LockState>(),
            Err(ParseError::Missing("num_lock"))
        );
        assert_eq!(
            "caps_lock=1 num_lock=off".parse::<LockState>(),
            Err(ParseError::BadValue("caps_lock=1".into()))
        );
        assert_eq!(
            "caps_lock num_lock=off".parse::<LockState>(),
            Err(ParseError::BadToken("caps_lock".into()))
        );
    }

    #[test]
    fn socket_path_can_be_overridden() {
        assert_eq!(socket_path_from(None), PathBuf::from(DEFAULT_SOCKET_PATH));
        assert_eq!(
            socket_path_from(Some(OsString::new())),
            PathBuf::from(DEFAULT_SOCKET_PATH)
        );
        assert_eq!(
            socket_path_from(Some("/tmp/test.sock".into())),
            PathBuf::from("/tmp/test.sock")
        );
    }
}
