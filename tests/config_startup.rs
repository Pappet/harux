//! #233: a broken configuration stops the start — the real binary exits
//! with an error naming the problem instead of running on defaults.

use std::process::Command;
use std::time::{Duration, Instant};

fn run_with(toml: Option<&str>, env: &[(&str, &str)]) -> (Option<i32>, String) {
    let dir = std::env::temp_dir().join(format!(
        "harux-config-{}-{}",
        std::process::id(),
        env.len() + toml.map_or(0, str::len)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    if let Some(toml) = toml {
        std::fs::write(dir.join("harux.toml"), toml).unwrap();
    }
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_harux"));
    cmd.current_dir(&dir)
        .env_remove("MLLP_PORT")
        .env_remove("WEB_PORT")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break Some(st);
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None; // still running: it started despite the bad config
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut stderr = String::new();
    use std::io::Read;
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (status.map(|s| s.code().unwrap_or(-1)), stderr)
}

#[test]
fn bad_value_type_stops_the_start() {
    let (code, stderr) = run_with(
        Some("[server]\nmllp_port = 12578\nweb_port = 18083\n[store]\nmax_messages = \"500\"\n"),
        &[],
    );
    assert!(
        matches!(code, Some(c) if c != 0),
        "exit {code:?}, stderr: {stderr}"
    );
    assert!(
        stderr.contains("harux.toml") && stderr.contains("max_messages"),
        "{stderr}"
    );
}

#[test]
fn typo_in_key_stops_the_start() {
    let (code, stderr) = run_with(Some("[store]\nmax_mesages = 500\n"), &[]);
    assert!(
        matches!(code, Some(c) if c != 0),
        "exit {code:?}, stderr: {stderr}"
    );
    assert!(stderr.contains("max_mesages"), "{stderr}");
}

#[test]
fn invalid_port_variable_stops_the_start() {
    let (code, stderr) = run_with(None, &[("MLLP_PORT", "abc")]);
    assert!(
        matches!(code, Some(c) if c != 0),
        "exit {code:?}, stderr: {stderr}"
    );
    assert!(stderr.contains("MLLP_PORT"), "{stderr}");
}
