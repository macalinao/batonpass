use std::ffi::OsString;

use bpaf::Bpaf;

/// Run a command with a share of the make jobserver's tokens.
///
/// baton takes up to N tokens from the jobserver in MAKEFLAGS (or
/// CARGO_MAKEFLAGS/MFLAGS), runs COMMAND with the count it got, and returns
/// the tokens when COMMAND exits. Its implicit token counts as one, so
/// `-n 4` takes 3 tokens from the pool.
#[derive(Debug, Clone, Bpaf)]
#[bpaf(options, version)]
pub struct Opts {
    /// Jobs to run with, counting the implicit token [default: nproc]
    #[bpaf(short('n'), long("jobs"), argument("N"))]
    pub jobs: Option<usize>,

    /// Start once M jobs are granted and take the rest only if free right
    /// now [default: N with -n, else 1]
    #[bpaf(long, argument("M"))]
    pub min: Option<usize>,

    /// Also export the job count as VAR, e.g. RAYON_NUM_THREADS (repeatable)
    #[bpaf(long("set"), argument("VAR"))]
    pub set: Vec<String>,

    /// With no jobserver in the environment, start one with N tokens and
    /// pass it to COMMAND
    #[bpaf(long, argument("N"))]
    pub serve: Option<usize>,

    /// Print the grant to stderr
    #[bpaf(short, long)]
    pub verbose: bool,

    /// Command to run; `{jobs}` in its arguments becomes the job count
    #[bpaf(positional("COMMAND"), strict, some("missing COMMAND after --"))]
    pub command: Vec<OsString>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Opts, String> {
        opts()
            .run_inner(args)
            .map_err(|e| e.unwrap_stderr())
    }

    #[test]
    fn parses_flags_and_command() {
        let o = parse(&[
            "-n", "4", "--min", "2", "--set", "A", "--set", "B", "--", "vitest", "-x",
        ])
        .expect("parses");
        assert_eq!(o.jobs, Some(4));
        assert_eq!(o.min, Some(2));
        assert_eq!(o.set, ["A", "B"]);
        assert_eq!(o.command, ["vitest", "-x"]);
    }

    #[test]
    fn command_needs_double_dash() {
        assert!(parse(&["-n", "2", "oxlint"]).is_err());
        assert!(parse(&["-n", "2"]).is_err());
    }
}
