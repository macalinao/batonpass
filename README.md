# batonpass

`baton` runs a command with a share of the GNU make jobserver's tokens. When several agents and parallel builds hit one machine at once, each tool sizes its pool from the CPU count, and together they run far more jobs than the machine has cores. make, cargo and ninja already share a jobserver: a pool of tokens, where you hold one per job you run. Tools like vitest, oxlint, tsc and turbo don't speak the protocol, but they all have a worker or thread knob. baton takes tokens on such a tool's behalf, sets the knob to the number of tokens it got, and returns the tokens when the tool exits.

It works with any jobserver in `MAKEFLAGS`, `CARGO_MAKEFLAGS` or `MFLAGS`, in both forms: `--jobserver-auth=fifo:PATH` and the older `R,W` file descriptors. That covers a recipe under `make -j`, a build script under cargo, and a system-wide jobserver like Gentoo's [steve](https://gitweb.gentoo.org/proj/steve.git) (`MAKEFLAGS=--jobserver-auth=fifo:/dev/steve`).

## Usage

```sh
baton -n 4 -- oxlint                          # BATON_JOBS=4 in its env
baton -n 4 -- vitest run --maxWorkers={jobs}  # {jobs} becomes the count
baton -n 8 --min 2 -- tsc -b                  # start with 2 if that's all that's free
baton -n 4 --set RAYON_NUM_THREADS -- ./crunch
baton -n 4 --set UV_THREADPOOL_SIZE -- node build.mjs
baton -n 6 --set GOMAXPROCS -- tsgo
```

In a turbo task (turbo strips `MAKEFLAGS` in strict env mode, so pass it through with `"globalPassThroughEnv": ["MAKEFLAGS", "CARGO_MAKEFLAGS"]` in `turbo.json`):

```json
{ "scripts": { "test": "baton -n 4 -- vitest run --maxWorkers={jobs}" } }
```

Without a jobserver, start one for a whole tree of commands, the way cargo does:

```sh
baton --serve 12 -- turbo run test   # tasks' batons share 12 tokens
```

- `-n N`: jobs to run with, counting baton's implicit token, so it takes N−1 tokens from the pool. Default: nproc.
- `--min M`: wait until M jobs are granted, then take more up to N only if they're free right now. Default: N with `-n` (wait for all), otherwise 1.
- `--set VAR`: also export the count as `VAR`. `BATON_JOBS` is always set.
- `--serve N`: with no jobserver in the env, create one with N tokens and pass it to the command in `MAKEFLAGS`. The command gets N jobs.
- `-v`: print the grant to stderr.

With no jobserver and no `--serve`, baton runs the command at once with `-n` (or nproc) jobs.

The command runs as a child. baton holds the tokens itself (steve accounts tokens per process), writes back the exact bytes it read (steve's tokens can be NUL), forwards SIGHUP/SIGINT/SIGQUIT/SIGTERM sent to it (not the ones a terminal already sent the whole process group), and exits with the command's code, or dies by the same signal. A signal while baton waits for tokens returns what it holds and exits.

Tokens are permission, not a cap the kernel enforces. A tool that ignores its knob can still use more cores. Tools that speak the jobserver themselves (make, cargo, ninja ≥ 1.13) don't need baton. With steve, `-n` above `--per-process-limit` waits forever; use `--min`.

## Compared with

- **stevie** (steve's client): `stevie cmd` runs a command holding one token, and it only talks to steve. baton takes N tokens from any jobserver and tells the command how many it got.
- **sem** (GNU parallel): a counting semaphore of its own, with one slot per command. It doesn't know about the jobserver that make and cargo use, or how many threads each command starts.
- **[heavy](https://docs.rs/crate/heavy)**: a machine-wide pool of flock slots, one per command, with nesting, timeouts and a CI bypass. It's separate from the jobserver, and a command holds one slot whatever its size. baton draws from the pool that make, cargo and steve already share, and sizes the command to its grant.

## Status

0.1, private. Unix only; tested on Linux. See [Going public](#going-public).

## Development

```sh
nix develop          # rust toolchain, treefmt, git hooks
cargo test           # unit tests + runs baton under real pipe and fifo jobservers
nix flake check      # formatting, hooks, package build with tests
nix fmt
```

Layout: one workspace crate, `crates/batonpass` (binary `baton`): `src/opts.rs` (bpaf args), `src/command.rs` (`{jobs}` and env), `src/main.rs` (tokens, child, signals), `tests/jobserver.rs`.

## Going public

The repo starts private. `release-plz.toml` is in place, but nothing publishes yet. When it goes public:

1. **release-plz workflow** (`.github/workflows/release-plz.yml`), on push to master, gated on `github.repository_owner == 'macalinao'`:
   - `release-plz-pr` (`command: release-pr`, `contents: write`, `pull-requests: write`) opens or updates the release PR. PRs created with `GITHUB_TOKEN` don't trigger CI, so use a GitHub App token or PAT if CI should run on them.
   - `release-plz-release` (`command: release`, `contents: write`, `id-token: write`) publishes unpublished versions, tags them and creates GitHub releases.
   - Checkout with `fetch-depth: 0` and `persist-credentials: false`; `dtolnay/rust-toolchain@stable`; `release-plz/action`.
2. **crates.io trusted publishing (OIDC)**: publish 0.1.0 by hand once (trusted publishing needs the crate to exist), then add a trusted publisher on crates.io (repo `macalinao/batonpass`, workflow `release-plz.yml`). In the release job, get a short-lived token with `rust-lang/crates-io-auth-action` (needs `id-token: write`) and pass it as `CARGO_REGISTRY_TOKEN` from the step output. Don't store a `CARGO_REGISTRY_TOKEN` secret.
3. **CI hardening** in `ci.yml`:
   - `cargo clippy --workspace --all-targets -- -D warnings`;
   - `cargo-deny` (licenses, bans, advisories, sources) with a `deny.toml`;
   - an MSRV job: set `rust-version` in `workspace.package` and check it with that toolchain (or `cargo-msrv verify`);
   - maybe `cargo-audit` on a schedule (cargo-deny's advisories check covers most of it);
   - a macOS runner, since baton is Unix-only and macOS pipes behave differently.
4. **Repo hygiene**: branch protection on master requiring CI, SECURITY.md, CONTRIBUTING.md, crates.io badge, `CHANGELOG.md` (release-plz writes it).
5. Flip visibility: `gh repo edit macalinao/batonpass --visibility public --accept-visibility-change-consequences`.

## License

Apache-2.0. See [LICENSE](LICENSE).
