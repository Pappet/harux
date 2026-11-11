//! #224: a failed `accept()` must not stop Harux.
//!
//! Runs the real binary with a file-descriptor limit of 64 (`ulimit -n` in a
//! shell, so no extra crate is needed) and opens more MLLP connections than it
//! can accept. The kernel completes the handshakes in the listen backlog, so
//! `accept()` fails with `EMFILE` inside Harux. Before the fix that ended the
//! process; now it must keep listening and serve again once descriptors are
//! free. Unix only: `ulimit` and SIGTERM.
#![cfg(unix)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn connect_when_up(port: u16) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => return s,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => panic!("Harux did not start listening on {port}: {e}"),
        }
    }
}

/// Send one framed message and read the reply up to `FS CR`.
fn exchange(s: &mut TcpStream, payload: &str) -> String {
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut frame = vec![0x0B];
    frame.extend_from_slice(payload.as_bytes());
    frame.extend_from_slice(&[0x1C, 0x0D]);
    s.write_all(&frame).unwrap();
    let mut reply = Vec::new();
    let mut byte = [0u8; 1];
    while !reply.ends_with(&[0x1C, 0x0D]) {
        s.read_exact(&mut byte).expect("reply");
        reply.push(byte[0]);
    }
    String::from_utf8_lossy(&reply).into_owned()
}

#[test]
fn accept_errors_do_not_stop_the_server() {
    let dir: PathBuf = std::env::temp_dir().join(format!("harux-accept-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log_path = dir.join("harux.log");
    let (mllp_port, web_port) = (free_port(), free_port());

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "ulimit -n 64 && exec '{}'",
            env!("CARGO_BIN_EXE_harux")
        ))
        .current_dir(&dir) // no harux.toml here: defaults + env
        .env("MLLP_PORT", mllp_port.to_string())
        .env("WEB_PORT", web_port.to_string())
        .env("RUST_LOG", "info")
        .stdout(Stdio::from(std::fs::File::create(&log_path).unwrap()))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(connect_when_up(mllp_port));

        // More connections than descriptors: accept() fails with EMFILE.
        let flood: Vec<TcpStream> = (0..80)
            .map(|_| TcpStream::connect(("127.0.0.1", mllp_port)).expect("connect"))
            .collect();
        std::thread::sleep(Duration::from_secs(1));
        assert!(
            child.try_wait().unwrap().is_none(),
            "Harux exited while accept() was failing"
        );
        drop(flood);
        std::thread::sleep(Duration::from_millis(800));

        // Descriptors are free again: a new connection is served.
        let mut s = connect_when_up(mllp_port);
        let reply = exchange(&mut s, "MSH|^~\\&|A|B|C|D|20240101||ADT^A01|AFTER|P|2.5\r");
        assert!(reply.contains("MSA|AA|AFTER"), "{reply:?}");
        drop(s);

        // SIGTERM ends it cleanly (#225).
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        let status = loop {
            if let Some(st) = child.try_wait().unwrap() {
                break st;
            }
            assert!(Instant::now() < deadline, "no exit after SIGTERM");
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(status.success(), "exit status {status:?}");

        let log = std::fs::read_to_string(&log_path).unwrap();
        assert!(
            log.contains("still listening"),
            "accept error not logged:\n{log}"
        );
        assert!(log.contains("Accepting MLLP connections again"), "{log}");
    }));

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
