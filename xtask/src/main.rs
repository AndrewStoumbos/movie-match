//! `cargo dev`: builds the server and the desktop app (release), starts the server,
//! and opens the app pointed at it through `MOVIE_MATCH_SERVER`. Closing the app
//! stops the server. The server reads `TMDB_API_KEY` from `.env` as usual.

use std::env;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode};
use std::thread;
use std::time::{Duration, Instant};

/// Where the server listens: this computer only.
const SERVER: &str = "127.0.0.1:8765";

/// How long to wait for the server to start listening.
const STARTUP: Duration = Duration::from_secs(15);

fn main() -> ExitCode {
    match dev() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn dev() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("xtask isn't inside the workspace")?;
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let built = Command::new(&cargo)
        .current_dir(root)
        .args([
            "build",
            "--release",
            "-p",
            "movie-match-server",
            "-p",
            "movie-match-app",
        ])
        .status()
        .map_err(|err| format!("could not run cargo: {err}"))?;
    if !built.success() {
        return Err("the build failed (is the app still open? close it and retry)".into());
    }

    let release = env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| root.join("target"), PathBuf::from)
        .join("release");
    let exe = |name: &str| release.join(format!("{name}{}", env::consts::EXE_SUFFIX));
    if listening() {
        return Err(format!(
            "something already listens on {SERVER} (an earlier server?); stop it first"
        ));
    }
    let mut server = Command::new(exe("movie-match-server"))
        .current_dir(root)
        .args(["--listen", SERVER])
        .spawn()
        .map_err(|err| format!("could not start the server: {err}"))?;
    let result = wait_for(&mut server).and_then(|()| {
        eprintln!("Opening the app. Close it to stop the server.");
        Command::new(exe("movie-match"))
            .current_dir(root)
            .env("MOVIE_MATCH_SERVER", format!("http://{SERVER}"))
            .status()
            .map(|status| eprintln!("The app closed ({status})."))
            .map_err(|err| format!("could not start the app: {err}"))
    });
    // The app is closed (or never opened): stop the server either way.
    let _ = server.kill();
    let _ = server.wait();
    eprintln!("Server stopped.");
    result
}

/// Whether anything accepts connections on [`SERVER`].
fn listening() -> bool {
    SERVER
        .parse::<SocketAddr>()
        .is_ok_and(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok())
}

/// Waits until the server accepts connections, failing if it exits first.
fn wait_for(server: &mut Child) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < STARTUP {
        if listening() {
            return Ok(());
        }
        if let Ok(Some(status)) = server.try_wait() {
            return Err(format!(
                "the server stopped ({status}); see its message above (no TMDB_API_KEY in \
                 .env?)"
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(format!("the server didn't start listening on {SERVER}"))
}
