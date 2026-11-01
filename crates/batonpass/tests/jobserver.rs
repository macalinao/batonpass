//! Runs `baton` under real jobservers: an anonymous pipe made by
//! `jobserver::Client::new` (the `R,W` form) and a named fifo holding NUL
//! tokens like Gentoo's steve (the `fifo:PATH` form).

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use jobserver::Client;

const TIMEOUT: Duration = Duration::from_secs(10);

/// `baton` with no jobserver in its environment, unless one is configured.
fn baton(client: Option<&Client>) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_baton"));
    for var in [
        "MAKEFLAGS",
        "CARGO_MAKEFLAGS",
        "MFLAGS",
        "BATON_VERBOSE",
        "BATON_REQUIRE",
    ] {
        cmd.env_remove(var);
    }
    if let Some(client) = client {
        client.configure_make(&mut cmd);
    }
    cmd.stdout(Stdio::piped());
    cmd
}

/// A fresh directory for marker files.
fn scratch() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "baton-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// A command that marks `started`, then waits for `go`.
fn gated(dir: &Path, extra: &str) -> Vec<String> {
    let started = dir.join("started");
    let go = dir.join("go");
    vec![
        "sh".into(),
        "-c".into(),
        format!(
            "echo $BATON_JOBS; {extra} touch '{}'; while [ ! -e '{}' ]; do sleep 0.02; done",
            started.display(),
            go.display()
        ),
    ]
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + TIMEOUT;
    while !path.exists() {
        assert!(Instant::now() < deadline, "timed out waiting for {path:?}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn finish(child: Child) -> Output {
    child.wait_with_output().expect("wait for baton")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// Bytes waiting in a pipe or fifo, i.e. free tokens.
fn queued(file: &File) -> usize {
    let mut n: libc::c_int = 0;
    let rc = unsafe { libc::ioctl(file.as_raw_fd(), libc::FIONREAD, &mut n) };
    assert_eq!(rc, 0, "FIONREAD");
    n as usize
}

#[test]
fn holds_tokens_while_running_and_returns_them() {
    let client = Client::new(4).expect("jobserver");
    let dir = scratch();
    let child = baton(Some(&client))
        .args(["-n", "3", "--"])
        .args(gated(&dir, ""))
        .spawn()
        .expect("spawn baton");

    wait_for(&dir.join("started"));
    // -n 3 is the implicit token plus two from the pool.
    assert_eq!(client.available().expect("available"), 2);

    File::create(dir.join("go")).expect("go");
    let out = finish(child);
    assert!(out.status.success());
    assert_eq!(stdout(&out), "3");
    assert_eq!(client.available().expect("available"), 4);
}

#[test]
fn fifo_jobserver_gets_back_the_exact_bytes() {
    let dir = scratch();
    let path = dir.join("fifo");
    let cpath = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).expect("path");
    assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0, "mkfifo");
    let mut fifo = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open fifo");
    // steve hands out NUL bytes; they must come back unchanged.
    fifo.write_all(&[0, 0, 0]).expect("fill fifo");

    let child = baton(None)
        .env(
            "MAKEFLAGS",
            format!("-j --jobserver-auth=fifo:{}", path.display()),
        )
        .args(["-n", "3", "--"])
        .args(gated(&dir, ""))
        .spawn()
        .expect("spawn baton");

    wait_for(&dir.join("started"));
    assert_eq!(queued(&fifo), 1);

    File::create(dir.join("go")).expect("go");
    let out = finish(child);
    assert!(out.status.success());
    assert_eq!(stdout(&out), "3");
    assert_eq!(queued(&fifo), 3);
    let mut tokens = [0xff; 3];
    fifo.read_exact(&mut tokens).expect("read tokens");
    assert_eq!(tokens, [0, 0, 0]);
}

#[test]
fn waits_while_the_pool_is_empty() {
    let client = Client::new(2).expect("jobserver");
    let a = client.acquire().expect("token");
    let b = client.acquire().expect("token");
    let dir = scratch();
    let started = dir.join("started");
    let mut child = baton(Some(&client))
        .args(["-n", "2", "--", "touch"])
        .arg(&started)
        .spawn()
        .expect("spawn baton");

    thread::sleep(Duration::from_millis(300));
    assert!(!started.exists(), "ran without a token");
    assert!(child.try_wait().expect("try_wait").is_none());

    drop(a);
    let out = finish(child);
    assert!(out.status.success());
    assert!(started.exists());
    assert_eq!(client.available().expect("available"), 1);
    drop(b);
    assert_eq!(client.available().expect("available"), 2);
}

#[test]
fn min_starts_with_fewer_tokens() {
    let client = Client::new(3).expect("jobserver");
    let _held = [
        client.acquire().expect("token"),
        client.acquire().expect("token"),
    ];
    let out = baton(Some(&client))
        .args([
            "-n",
            "4",
            "--min",
            "2",
            "--",
            "sh",
            "-c",
            "echo $BATON_JOBS",
        ])
        .output()
        .expect("run baton");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "2");
    assert_eq!(client.available().expect("available"), 1);
}

#[test]
fn passes_the_exit_code_through() {
    let client = Client::new(2).expect("jobserver");
    let out = baton(Some(&client))
        .args(["-n", "2", "--", "sh", "-c", "exit 7"])
        .output()
        .expect("run baton");
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(client.available().expect("available"), 2);

    let missing = baton(None)
        .args(["--", "baton-test-no-such-command"])
        .output()
        .expect("run baton");
    assert_eq!(missing.status.code(), Some(127));
}

#[test]
fn dies_by_the_commands_signal() {
    let client = Client::new(2).expect("jobserver");
    let out = baton(Some(&client))
        .args(["-n", "2", "--", "sh", "-c", "kill -KILL $$"])
        .output()
        .expect("run baton");
    assert_eq!(out.status.signal(), Some(libc::SIGKILL));
    assert_eq!(client.available().expect("available"), 2);
}

#[test]
fn forwards_sigterm_and_returns_tokens() {
    let client = Client::new(3).expect("jobserver");
    let dir = scratch();
    let child = baton(Some(&client))
        .args(["-n", "3", "--"])
        .args(gated(&dir, "trap 'exit 42' TERM;"))
        .spawn()
        .expect("spawn baton");

    wait_for(&dir.join("started"));
    assert_eq!(client.available().expect("available"), 1);
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };

    let out = finish(child);
    assert_eq!(out.status.code(), Some(42));
    assert_eq!(client.available().expect("available"), 3);
}

#[test]
fn signal_while_waiting_returns_tokens() {
    let client = Client::new(3).expect("jobserver");
    let held = [
        client.acquire().expect("token"),
        client.acquire().expect("token"),
    ];
    // Needs two tokens, gets one, then blocks.
    let child = baton(Some(&client))
        .args(["-n", "3", "--", "true"])
        .spawn()
        .expect("spawn baton");

    let deadline = Instant::now() + TIMEOUT;
    while client.available().expect("available") != 0 {
        assert!(Instant::now() < deadline, "baton never took the free token");
        thread::sleep(Duration::from_millis(10));
    }
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };

    let out = finish(child);
    assert_eq!(out.status.signal(), Some(libc::SIGTERM));
    assert_eq!(client.available().expect("available"), 1);
    drop(held);
}

#[test]
fn without_a_jobserver_uses_n() {
    let out = baton(None)
        .args([
            "-n",
            "5",
            "--set",
            "RAYON_NUM_THREADS",
            "--",
            "sh",
            "-c",
            "echo $BATON_JOBS $RAYON_NUM_THREADS $1",
            "sh",
            "--maxWorkers={jobs}",
        ])
        .output()
        .expect("run baton");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "5 5 --maxWorkers=5");
}

#[test]
fn serve_shares_a_new_jobserver_with_the_command() {
    let inner = env!("CARGO_BIN_EXE_baton");
    let out = baton(None)
        .args(["--serve", "3", "--", inner, "-n", "3", "--"])
        .args(["sh", "-c", "echo $BATON_JOBS; echo $MAKEFLAGS"])
        .output()
        .expect("run baton");
    assert!(out.status.success());
    let text = stdout(&out);
    let mut lines = text.lines();
    // The pool has 3 tokens and the outer baton holds one for the command's
    // implicit token, so the inner baton gets all 3 jobs.
    assert_eq!(lines.next(), Some("3"));
    assert!(
        lines
            .next()
            .is_some_and(|l| l.contains("--jobserver-auth="))
    );
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn require_fails_without_a_jobserver() {
    let dir = scratch();
    let ran = dir.join("ran");
    let flag = baton(None)
        .args(["--require", "--", "touch"])
        .arg(&ran)
        .output()
        .expect("run baton");
    let env = baton(None)
        .env("BATON_REQUIRE", "1")
        .args(["--", "touch"])
        .arg(&ran)
        .output()
        .expect("run baton");
    for out in [flag, env] {
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr(&out).contains("--require: no jobserver"),
            "{}",
            stderr(&out)
        );
    }
    assert!(!ran.exists(), "ran without a jobserver");

    let off = baton(None)
        .env("BATON_REQUIRE", "0")
        .args(["--", "true"])
        .output()
        .expect("run baton");
    assert!(off.status.success());
}

#[test]
fn require_runs_with_a_jobserver() {
    let client = Client::new(2).expect("jobserver");
    let out = baton(Some(&client))
        .args(["--require", "-n", "2", "--", "sh", "-c", "echo $BATON_JOBS"])
        .output()
        .expect("run baton");
    assert!(out.status.success());
    assert_eq!(stdout(&out), "2");
}

#[test]
fn baton_verbose_env_prints_the_grant() {
    let quiet = baton(None)
        .args(["-n", "2", "--", "true"])
        .output()
        .expect("run baton");
    assert!(!stderr(&quiet).contains("jobs="));

    let out = baton(None)
        .env("BATON_VERBOSE", "1")
        .args(["-n", "2", "--", "true"])
        .output()
        .expect("run baton");
    assert!(out.status.success());
    assert!(
        stderr(&out).contains("baton: jobs=2 (no jobserver)"),
        "{}",
        stderr(&out)
    );
}
