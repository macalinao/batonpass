mod command;
mod opts;

use std::ffi::OsStr;
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use jobserver::{Acquired, Client, FromEnvErrorKind};
use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
use signal_hook::iterator::SignalsInfo;
use signal_hook::iterator::exfiltrator::WithOrigin;
use signal_hook::low_level::siginfo::{Cause, Origin};

use crate::command::{job_env, substitute};
use crate::opts::Opts;

/// Gentoo's system-wide jobserver.
const STEVE: &str = "/dev/steve";

/// Signals passed on to the command. SIGUSR1 is left alone: the jobserver
/// crate's helper thread uses it to interrupt a blocked read.
const FORWARDED: [i32; 4] = [SIGHUP, SIGINT, SIGQUIT, SIGTERM];

enum Event {
    Token(io::Result<Acquired>),
    Signal(Origin),
    Exited(io::Result<ExitStatus>),
}

/// How baton should end, decided after every token has been written back.
enum Outcome {
    Code(i32),
    Signal(i32),
}

/// Where the job count comes from.
enum Source {
    /// A jobserver from MAKEFLAGS and friends.
    Env(Client),
    /// A jobserver baton created with `--serve`, and its size.
    Served(Client, usize),
    /// No jobserver: run with `-n` jobs.
    Alone,
}

fn main() {
    // Before anything else opens fds, so the inherited ones are ours.
    let from_env = unsafe { Client::from_env_ext(true) };
    let opts = opts::opts().run();
    let outcome = match run(opts, from_env.client) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("baton: {e}");
            Outcome::Code(1)
        }
    };
    match outcome {
        Outcome::Code(code) => std::process::exit(code),
        Outcome::Signal(sig) => {
            // Die the way the command did, so callers see the signal; but
            // don't leave a core file of baton itself.
            unsafe {
                let zero = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                libc::setrlimit(libc::RLIMIT_CORE, &zero);
            }
            let _ = signal_hook::low_level::emulate_default_handler(sig);
            std::process::exit(128 + sig);
        }
    }
}

/// Whether a signal was sent by a process (kill(2) and friends) rather than
/// by the kernel, e.g. for ^C on the terminal.
fn sent(origin: &Origin) -> bool {
    match origin.cause {
        Cause::Sent(_) => true,
        // macOS leaves si_code unset; a sender pid still means kill(2).
        Cause::Unknown => origin.process.is_some_and(|p| p.pid != 0),
        _ => false,
    }
}

/// Number of jobs wanted and the number to wait for, from `-n` and `--min`.
fn plan(jobs: Option<usize>, min: Option<usize>, nproc: usize) -> (usize, usize) {
    let n = jobs.unwrap_or(nproc).max(1);
    // Without -n, waiting for nproc tokens could take forever (steve caps
    // tokens per process), so start with whatever is free.
    let min = min.unwrap_or(if jobs.is_some() { n } else { 1 });
    (n, min.clamp(1, n))
}

/// Whether an env flag like `BATON_VERBOSE` is on: set, and not empty, `0`,
/// `false` or `no`.
fn env_flag(value: Option<&OsStr>) -> bool {
    value.is_some_and(|v| {
        !(v.is_empty()
            || v == "0"
            || v.eq_ignore_ascii_case("false")
            || v.eq_ignore_ascii_case("no"))
    })
}

/// The warning for a steve fifo that MAKEFLAGS doesn't point at.
fn steve_warning(no_jobserver: bool, steve_exists: bool) -> Option<String> {
    (no_jobserver && steve_exists).then(|| {
        format!(
            "baton: warning: {STEVE} exists but MAKEFLAGS has no jobserver; \
             set MAKEFLAGS=--jobserver-auth=fifo:{STEVE} to share it"
        )
    })
}

fn run(mut opts: Opts, env: Result<Client, jobserver::FromEnvError>) -> Result<Outcome, String> {
    opts.verbose |= env_flag(std::env::var_os("BATON_VERBOSE").as_deref());
    opts.require |= env_flag(std::env::var_os("BATON_REQUIRE").as_deref());
    let nproc = thread::available_parallelism().map_or(1, |n| n.get());
    let (n, min) = plan(opts.jobs, opts.min, nproc);

    let (tx, rx) = mpsc::channel();
    let mut signals = SignalsInfo::<WithOrigin>::new(FORWARDED)
        .map_err(|e| format!("installing signal handlers: {e}"))?;
    let signals_handle = signals.handle();
    {
        let tx = tx.clone();
        thread::spawn(move || {
            for origin in signals.forever() {
                if tx.send(Event::Signal(origin)).is_err() {
                    break;
                }
            }
        });
    }

    let source = match env {
        Ok(client) => Source::Env(client),
        Err(e) => {
            let none = matches!(
                e.kind(),
                FromEnvErrorKind::NoEnvVar | FromEnvErrorKind::NoJobserver
            );
            if let Some(warning) = steve_warning(none, Path::new(STEVE).exists()) {
                eprintln!("{warning}");
            }
            if opts.require {
                return Err(if none {
                    "--require: no jobserver in MAKEFLAGS, CARGO_MAKEFLAGS or MFLAGS".into()
                } else {
                    format!("--require: unusable jobserver: {e}")
                });
            }
            if !none {
                eprintln!("baton: ignoring jobserver: {e}");
            }
            match opts.serve {
                Some(size) => {
                    let size = size.max(1);
                    let client =
                        Client::new(size).map_err(|e| format!("creating a jobserver: {e}"))?;
                    // The command's implicit token, as cargo does.
                    client
                        .acquire_raw()
                        .map_err(|e| format!("acquiring from our jobserver: {e}"))?;
                    Source::Served(client, size)
                }
                None => Source::Alone,
            }
        }
    };

    let mut held = Vec::new();
    let jobs = match &source {
        Source::Env(client) => {
            if let Some(sig) = acquire(client, n, min, &mut held, &tx, &rx)? {
                return Ok(Outcome::Signal(sig));
            }
            held.len() + 1
        }
        Source::Served(_, size) => *size,
        Source::Alone => n,
    };
    if opts.verbose {
        match &source {
            Source::Env(_) => eprintln!("baton: jobs={jobs} ({} tokens taken)", held.len()),
            Source::Served(..) => eprintln!("baton: jobs={jobs} (serving {jobs} tokens)"),
            Source::Alone => eprintln!("baton: jobs={jobs} (no jobserver)"),
        }
    }

    let (program, args) = opts
        .command
        .split_first()
        .ok_or("missing COMMAND after --")?;
    let mut cmd = Command::new(program);
    cmd.args(args.iter().map(|a| substitute(a, jobs)));
    cmd.envs(job_env(jobs, &opts.set));
    match &source {
        // Keep the caller's MAKEFLAGS; only make the fds inheritable.
        Source::Env(client) => client.configure(&mut cmd),
        Source::Served(client, _) => client.configure_make(&mut cmd),
        Source::Alone => {}
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("baton: {}: {e}", program.to_string_lossy());
            let code = if e.kind() == io::ErrorKind::NotFound {
                127
            } else {
                126
            };
            return Ok(Outcome::Code(code));
        }
    };
    let pid = child.id() as libc::pid_t;
    {
        let tx = tx.clone();
        thread::spawn(move || {
            let _ = tx.send(Event::Exited(child.wait()));
        });
    }

    let status = loop {
        match rx.recv() {
            Ok(Event::Signal(origin)) => {
                // Signals from the terminal already reach the command through
                // its process group; forwarding those would deliver them twice.
                if sent(&origin) {
                    unsafe { libc::kill(pid, origin.signal) };
                }
            }
            Ok(Event::Exited(status)) => break status.map_err(|e| format!("waiting: {e}"))?,
            Ok(Event::Token(_)) => {}
            Err(_) => return Err("event channel closed".into()),
        }
    };
    signals_handle.close();
    drop(held);

    Ok(match (status.code(), status.signal()) {
        (Some(code), _) => Outcome::Code(code),
        (None, Some(sig)) => Outcome::Signal(sig),
        (None, None) => Outcome::Code(1),
    })
}

/// Takes tokens until `min` jobs are held (blocking), then more up to `n` if
/// they are free right now. The implicit token counts as one job. Returns the
/// signal if one arrived while waiting.
fn acquire(
    client: &Client,
    n: usize,
    min: usize,
    held: &mut Vec<Acquired>,
    tx: &Sender<Event>,
    rx: &Receiver<Event>,
) -> Result<Option<i32>, String> {
    if min > 1 {
        let tx = tx.clone();
        // A helper thread does the blocking reads so a signal can cut the
        // wait short; tokens it takes after that are dropped (returned).
        let helper = client
            .clone()
            .into_helper_thread(move |token| {
                let _ = tx.send(Event::Token(token));
            })
            .map_err(|e| format!("starting jobserver helper: {e}"))?;
        for _ in 1..min {
            helper.request_token();
        }
        while held.len() + 1 < min {
            match rx.recv() {
                Ok(Event::Token(Ok(token))) => held.push(token),
                Ok(Event::Token(Err(e))) => return Err(format!("acquiring a token: {e}")),
                Ok(Event::Signal(origin)) => return Ok(Some(origin.signal)),
                Ok(Event::Exited(_)) => {}
                Err(_) => return Err("event channel closed".into()),
            }
        }
    }
    while held.len() + 1 < n {
        match client.try_acquire() {
            Ok(Some(token)) => held.push(token),
            Ok(None) => break,
            // Pipe jobservers on some platforms can't be read without
            // blocking; start with what we have.
            Err(e) if e.kind() == io::ErrorKind::Unsupported => break,
            Err(e) => return Err(format!("acquiring a token: {e}")),
        }
    }
    // A signal that arrived while we were taking tokens: stop before starting.
    while let Ok(event) = rx.try_recv() {
        if let Event::Signal(origin) = event {
            return Ok(Some(origin.signal));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_flag_values() {
        for on in ["1", "true", "yes", "TRUE"] {
            assert!(env_flag(Some(OsStr::new(on))), "{on}");
        }
        for off in ["", "0", "false", "No"] {
            assert!(!env_flag(Some(OsStr::new(off))), "{off}");
        }
        assert!(!env_flag(None));
    }

    #[test]
    fn warns_about_unused_steve() {
        assert!(steve_warning(true, true).is_some_and(|w| w.contains("fifo:/dev/steve")));
        assert_eq!(steve_warning(true, false), None);
        assert_eq!(steve_warning(false, true), None);
    }

    #[test]
    fn plan_defaults_to_nproc_without_waiting() {
        assert_eq!(plan(None, None, 8), (8, 1));
    }

    #[test]
    fn plan_waits_for_all_with_n() {
        assert_eq!(plan(Some(4), None, 8), (4, 4));
    }

    #[test]
    fn plan_clamps_min() {
        assert_eq!(plan(Some(4), Some(2), 8), (4, 2));
        assert_eq!(plan(Some(4), Some(9), 8), (4, 4));
        assert_eq!(plan(Some(4), Some(0), 8), (4, 1));
        assert_eq!(plan(Some(0), None, 8), (1, 1));
    }
}
