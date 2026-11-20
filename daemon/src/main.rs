mod server;

use evdev::{Device, EventType, KeyCode, LedCode};
use lock_keys_protocol::LockState;
use log::{debug, error, info, warn};
use server::Server;
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::{fs, io, process, thread};

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let from_stdin = match std::env::args().nth(1).as_deref() {
        None => false,
        Some("--stdin") => true,
        Some(_) => {
            eprintln!("usage: lock-keys-daemon [--stdin]");
            eprintln!("  --stdin  read states like `caps_lock=on num_lock=off` from stdin instead of a keyboard (for testing)");
            process::exit(2);
        }
    };

    let path = lock_keys_protocol::socket_path();
    let result = if from_stdin {
        run_stdin(&path)
    } else {
        run_keyboard(&path)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!("{err}");
            ExitCode::FAILURE
        }
    }
}

fn find_keyboard() -> io::Result<Option<(PathBuf, Device)>> {
    for entry in fs::read_dir("/dev/input")? {
        let entry = entry?;
        let path = entry.path();

        let name = path.file_name().unwrap().to_string_lossy();

        if !name.starts_with("event") {
            continue;
        }

        let device = match Device::open(&path) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let is_keyboard = device.supported_keys().is_some_and(|keys| {
            keys.contains(KeyCode::KEY_CAPSLOCK) || keys.contains(KeyCode::KEY_NUMLOCK)
        });

        if is_keyboard {
            return Ok(Some((path, device))); // Found a keyboard, stop searching
        }
    }

    Ok(None)
}

fn led_state(dev: &Device) -> io::Result<LockState> {
    let leds = dev.get_led_state()?;
    Ok(LockState {
        caps_lock: leds.contains(LedCode::LED_CAPSL),
        num_lock: leds.contains(LedCode::LED_NUML),
    })
}

fn listen(path: &Path, initial: LockState) -> io::Result<Server> {
    let server = Server::bind(path, initial)?;
    info!("Listening on {}", path.display());

    // Remove the socket on Ctrl+C or `systemctl stop`.
    let mut signals = Signals::new([SIGINT, SIGTERM])?;
    let socket = path.to_path_buf();
    thread::spawn(move || {
        if signals.forever().next().is_some() {
            let _ = fs::remove_file(&socket);
            process::exit(0);
        }
    });

    Ok(server)
}

fn run_keyboard(path: &Path) -> io::Result<()> {
    let (dev_path, mut dev) = find_keyboard()?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "No keyboard device found (reading /dev/input needs root or the input group)",
        )
    })?;
    info!("Keyboard found at: {}", dev_path.display());

    let state = led_state(&dev)?;
    info!("{state}");
    let server = listen(path, state)?;

    loop {
        let mut changed = false;
        for ev in dev.fetch_events()? {
            // The compositor sets the LEDs after the key press, which arrives here
            // as an LED event, so both are a reason to read the LED state again.
            let relevant = match ev.event_type() {
                EventType::LED => true,
                EventType::KEY => {
                    let key = KeyCode(ev.code());
                    key == KeyCode::KEY_NUMLOCK || key == KeyCode::KEY_CAPSLOCK
                }
                _ => false,
            };
            if relevant {
                debug!("{:?}", ev);
                changed = true;
            }
        }
        if changed {
            let state = led_state(&dev)?;
            debug!("{state}");
            server.publish(state);
        }
    }
}

fn run_stdin(path: &Path) -> io::Result<()> {
    let server = listen(path, LockState::default())?;
    info!("Test mode: reading states from stdin");

    for line in io::stdin().lines() {
        let line = line?;
        match line.parse() {
            Ok(state) => {
                debug!("{state}");
                server.publish(state);
            }
            Err(err) => warn!("ignoring {line:?}: {err}"),
        }
    }

    Ok(())
}
