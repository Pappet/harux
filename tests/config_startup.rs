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

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Tests that start a server take this lock, so they don't pick ports for
/// each other (#255 review).
static SERVER_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Start the binary with the `harux.toml` that `toml(mllp_port, web_port)`
/// returns, wait until both listeners answer on `host`, run `check` and
/// return Harux's stderr. `free_port()` releases a port before Harux binds
/// it, so if another process takes it in between (Harux exits with
/// "cannot listen"), the start is retried on new ports. The process is
/// killed afterwards in any case.
fn run_until(
    tag: &str,
    host: &str,
    toml: impl Fn(u16, u16) -> String,
    check: impl FnOnce(u16, u16),
) -> String {
    let _serial = SERVER_TESTS.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("harux-startup-{tag}-{}", std::process::id()));
    let stderr_path = dir.join("stderr.txt");
    for attempt in 1..=3 {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("blocker"), "a file where a directory is expected").unwrap();
        let (mllp, web) = (free_port(), free_port());
        std::fs::write(dir.join("harux.toml"), toml(mllp, web)).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_harux"))
            .current_dir(&dir)
            .env_remove("MLLP_PORT")
            .env_remove("WEB_PORT")
            .env_remove("RUST_LOG")
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(&stderr_path).unwrap())
            .spawn()
            .unwrap();
        let up = wait_until_listening(&mut child, host, [mllp, web]);
        let stderr = || std::fs::read_to_string(&stderr_path).unwrap_or_default();
        if !up {
            let _ = child.kill();
            let _ = child.wait();
            let stderr = stderr();
            if attempt < 3 && stderr.contains("cannot listen") {
                continue; // a port was taken in between
            }
            let _ = std::fs::remove_dir_all(&dir);
            panic!("Harux did not come up on {host}:{mllp}/{web}; stderr:\n{stderr}");
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            check(mllp, web);
            assert!(child.try_wait().unwrap().is_none(), "Harux exited");
        }));
        let _ = child.kill();
        let _ = child.wait();
        let stderr = stderr();
        let _ = std::fs::remove_dir_all(&dir);
        if let Err(e) = result {
            eprintln!("stderr of Harux:\n{stderr}");
            std::panic::resume_unwind(e);
        }
        return stderr;
    }
    unreachable!()
}

/// Whether all `ports` answer on `host` before the child exits or 10 s pass.
fn wait_until_listening(child: &mut std::process::Child, host: &str, ports: [u16; 2]) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    for port in ports {
        while std::net::TcpStream::connect((host, port)).is_err() {
            if child.try_wait().unwrap().is_some() || Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    true
}

/// #234: a log file that can't be created used to panic at startup (exit
/// code 101). Harux now starts, serves, and says why it logs to stdout only.
#[test]
fn unwritable_log_file_does_not_stop_the_start() {
    let stderr = run_until(
        "log",
        "127.0.0.1",
        |mllp, web| {
            format!(
                "[server]\nmllp_port = {mllp}\nweb_port = {web}\n\
                 [logging]\nfile = \"blocker/harux.log\"\n"
            )
        },
        |_, _| {},
    );
    assert!(
        stderr.contains("blocker/harux.log") && stderr.contains("logging to stdout only"),
        "{stderr}"
    );
    assert!(!stderr.contains("panicked"), "{stderr}");
}

/// #237: with `127.0.0.1` the listeners answer on that address only. On
/// Linux all of 127.0.0.0/8 is loopback, so 127.0.0.2 reaches this machine
/// but not a listener bound to 127.0.0.1.
#[cfg(target_os = "linux")]
#[test]
fn bind_addresses_restrict_the_listeners() {
    run_until(
        "bind",
        "127.0.0.1",
        |mllp, web| {
            format!(
                "[server]\nmllp_port = {mllp}\nweb_port = {web}\n\
                 mllp_bind = \"127.0.0.1\"\nweb_bind = \"127.0.0.1\"\n"
            )
        },
        |mllp, web| {
            for port in [mllp, web] {
                assert!(
                    std::net::TcpStream::connect(("127.0.0.2", port)).is_err(),
                    "port {port} answers on 127.0.0.2 although bound to 127.0.0.1"
                );
            }
        },
    );
}
