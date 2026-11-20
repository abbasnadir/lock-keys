# lock-keys

A Rust workspace for showing Caps Lock and Num Lock state on the COSMIC desktop (Pop!_OS): a small daemon that watches the keyboard through evdev, and two COSMIC panel applets, one per key.

This is an early, unfinished project. The daemon reads the Caps Lock and Num Lock LEDs and sends their state over a Unix socket. Each applet follows that socket and shows its “on” or “off” icon.

![The four panel icons](docs/icons.png)

*The applets’ icons from `capslock/resources/` and `numslock/resources/`, rendered in a browser. This is not a screenshot of the panel: these demos were made on a Hyprland machine without COSMIC, so the applets couldn’t be shown running.*

## What works today

- **Daemon** (`daemon/`): scans `/dev/input/event*` and opens the first device that reports a Caps Lock or Num Lock key. It reads the keyboard LEDs at start and again after every Caps Lock or Num Lock key event or LED event, and sends each change to every connected applet.
- **Applets** (`capslock/`, `numslock/`): built on [libcosmic](https://github.com/pop-os/libcosmic)’s applet template. Each puts an icon button in the panel that opens a popup. The icon is the “on” icon while its key is on, and the dimmed “off” icon while it is off or while the daemon can’t be reached. The popup says “Caps Lock is on”, “Caps Lock is off” or “lock-keys-daemon is not running” (Num Lock for the other applet). An applet that loses the daemon tries again every second.
- **systemd unit** (`daemon/lock-keys.service`): runs `/usr/bin/lock-keys-daemon` as root, since reading `/dev/input` needs root or membership of the `input` group, and creates `/run/lock-keys` for the socket.

## How it works

- `daemon/src/main.rs` uses the [`evdev`](https://crates.io/crates/evdev) crate. A device counts as a keyboard if `supported_keys()` contains `KEY_CAPSLOCK` or `KEY_NUMLOCK`. Devices it can’t open are skipped. The state comes from the LEDs (`LED_CAPSL`, `LED_NUML`), because the compositor sets them after the key press.
- `daemon/src/server.rs` listens on `/run/lock-keys/lock-keys.sock` (or `$LOCK_KEYS_SOCKET`) with mode 0666, so applets running as the desktop user can connect to a daemon running as root. Each client gets the current state when it connects and then one line per change:

  ```text
  caps_lock=on num_lock=off
  ```

  A client that disconnects or stops reading is dropped. The daemon removes the socket when it stops (Ctrl+C or `systemctl stop`), and replaces a socket file left behind by a daemon that was killed. It refuses to start if another daemon already answers on the socket.
- `protocol/` (`lock-keys-protocol`) holds the shared parts: `LockState`, parsing and formatting of the line above, the socket path, and with the `client` feature an async `watch` function that the applets use to follow the daemon and reconnect.
- `capslock/src/app.rs` and `numslock/src/app.rs` are the same code apart from the app ID, the icons and which key they show. The icons are SVGs built into the binary with `include_bytes!`, so nothing is loaded from disk at run time. A subscription runs `watch` and turns each update into a message for `update`.
- The daemon logs through `env_logger` (default level `info`; the systemd unit sets `RUST_LOG=debug`, which also logs every key and LED event).

## Tech stack

Rust (workspace with resolver 3), `evdev` 0.13, tokio for the applets’ socket client, libcosmic (from git) with the `applet`, `wayland`, `tokio` and `dbus-config` features, Fluent (`i18n-embed`) for strings, `just` recipes from the COSMIC applet template.

## Build and run

Needs Rust (built here with cargo 1.98.1) and the system libraries libcosmic needs (on Pop!_OS these come with the COSMIC development packages).

```bash
cargo build -p lock-keys-daemon
cargo build -p capslock -p numslock
cargo test --workspace
```

All three commands ran cleanly here (17 tests). The first applet build fetches and compiles libcosmic, which took about 80 seconds here.

The daemon must be able to read `/dev/input`. Run it as a normal user who isn’t in the `input` group and it can’t open any device, so it stops straight away. This is real output from that case:

```text
$ ./target/debug/lock-keys-daemon
[2026-11-20T16:16:35Z ERROR lock_keys_daemon] No keyboard device found (reading /dev/input needs root or the input group)
```

To use the keyboard, run it with access to the input devices, for example `sudo ./target/debug/lock-keys-daemon`. It logs `Keyboard found at: /dev/input/eventN`, the current state and `Listening on /run/lock-keys/lock-keys.sock`.

### Test mode

`lock-keys-daemon --stdin` reads states from standard input instead of a keyboard, so the socket and the applets can be tried without root and without touching the real keyboard state. Point it at any socket path with `LOCK_KEYS_SOCKET`, and start the applet with the same variable. This is a real session. The three state lines are what was written to the daemon’s input, and the second block is what a small client connected to the socket received:

```text
$ LOCK_KEYS_SOCKET=$XDG_RUNTIME_DIR/lock-keys.sock ./target/debug/lock-keys-daemon --stdin
[2026-11-20T16:16:35Z INFO  lock_keys_daemon] Listening on /run/user/1000/lock-keys.sock
[2026-11-20T16:16:35Z INFO  lock_keys_daemon] Test mode: reading states from stdin
caps_lock=on num_lock=off
caps_lock=on num_lock=off
caps_lock=on num_lock=on
```

```text
client got: caps_lock=off num_lock=off
client got: caps_lock=on num_lock=off
client got: caps_lock=on num_lock=on
```

The repeated line wasn’t sent again. `daemon/tests/stdin_mode.rs` runs the same flow with the client the applets use.

### Installing

Each of `daemon/`, `capslock/` and `numslock/` has a `justfile`. `just` builds a release binary into the workspace’s `target/`, and `sudo just install` installs:

- `daemon/`: `/usr/bin/lock-keys-daemon` and `/usr/lib/systemd/system/lock-keys.service` (then `sudo systemctl enable --now lock-keys`)
- `capslock/`, `numslock/`: `/usr/bin/capslock` or `/usr/bin/numslock`, the `.desktop` file, the metainfo and the “on” icon as the app icon

`just rootdir=/some/dir install` installs under another root instead. That is how the recipes were checked here; the applets couldn’t be added to a COSMIC panel on this machine.

## Known issues

- The daemon only watches the first matching device, so a second keyboard (or a virtual one listed earlier) is missed.
- The daemon reads the keyboard LEDs. On a keyboard without them, or if the compositor doesn’t set them, the reported state doesn’t change.
- The applets’ icons are drawn in white, so they are hard to see on a light panel.
- The real keyboard path (evdev and LEDs) wasn’t run for this README, because this machine’s user has no access to `/dev/input`. The socket, the protocol and the applets’ client were tested through test mode.

## Project structure

```text
Cargo.toml              workspace: daemon, protocol, capslock, numslock
daemon/
  src/main.rs           evdev keyboard scan, LED reading, --stdin test mode
  src/server.rs         Unix socket that sends the state to applets
  tests/stdin_mode.rs   runs the daemon binary with the applets' client
  lock-keys.service     systemd unit (runs as root)
  justfile              build and install recipes
protocol/               state line format, socket path, async client
capslock/               COSMIC applet for Caps Lock
  src/app.rs            applet model, view, popup and daemon subscription
  resources/            capslockon.svg, capslockoff.svg, .desktop, metainfo
  i18n/en/              Fluent strings
  justfile              build and install recipes
numslock/               the same applet for Num Lock
docs/icons.png          the icons shown above
```
