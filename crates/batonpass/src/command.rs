use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};

const PLACEHOLDER: &[u8] = b"{jobs}";

/// Replaces every `{jobs}` in `arg` with `jobs`.
pub fn substitute(arg: &OsStr, jobs: usize) -> OsString {
    let bytes = arg.as_bytes();
    if !bytes.windows(PLACEHOLDER.len()).any(|w| w == PLACEHOLDER) {
        return arg.to_owned();
    }
    let jobs = jobs.to_string();
    let mut out = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while !rest.is_empty() {
        if rest.starts_with(PLACEHOLDER) {
            out.extend_from_slice(jobs.as_bytes());
            rest = &rest[PLACEHOLDER.len()..];
        } else {
            out.push(rest[0]);
            rest = &rest[1..];
        }
    }
    OsString::from_vec(out)
}

/// Environment variables carrying the job count: `BATON_JOBS` plus `extra`.
pub fn job_env(jobs: usize, extra: &[String]) -> Vec<(String, String)> {
    std::iter::once("BATON_JOBS")
        .chain(extra.iter().map(String::as_str))
        .map(|k| (k.to_owned(), jobs.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_placeholder() {
        assert_eq!(substitute(OsStr::new("--maxWorkers={jobs}"), 4), "--maxWorkers=4");
        assert_eq!(substitute(OsStr::new("{jobs}{jobs}"), 12), "1212");
        assert_eq!(substitute(OsStr::new("{job}"), 3), "{job}");
        assert_eq!(substitute(OsStr::new("plain"), 3), "plain");
    }

    #[test]
    fn keeps_non_utf8_bytes() {
        let arg = OsStr::from_bytes(b"\xff-{jobs}");
        assert_eq!(substitute(arg, 2).as_bytes(), b"\xff-2");
    }

    #[test]
    fn job_env_includes_baton_jobs() {
        let env = job_env(3, &["RAYON_NUM_THREADS".into()]);
        assert_eq!(
            env,
            [
                ("BATON_JOBS".into(), "3".into()),
                ("RAYON_NUM_THREADS".into(), "3".into())
            ]
        );
    }
}
