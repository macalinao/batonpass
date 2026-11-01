# batonpass

[![crates.io](https://img.shields.io/crates/v/batonpass.svg)](https://crates.io/crates/batonpass) [![CI](https://github.com/macalinao/batonpass/actions/workflows/ci.yml/badge.svg)](https://github.com/macalinao/batonpass/actions/workflows/ci.yml)

`baton` runs a command with a share of the GNU make jobserver's tokens. When several agents and parallel builds hit one machine at once, each tool sizes its pool from the CPU count, and together they run far more jobs than the machine has cores. make, cargo and ninja already share a jobserver: a pool of tokens, where you hold one per job you run. Tools like vitest, oxlint, tsc and turbo don't speak the protocol, but they all have a worker or thread knob. baton takes tokens on such a tool's behalf, sets the knob to the number of tokens it got, and returns the tokens when the tool exits.

It works with any jobserver in `MAKEFLAGS`, `CARGO_MAKEFLAGS` or `MFLAGS`, in both forms: `--jobserver-auth=fifo:PATH` and the older `R,W` file descriptors. That covers a recipe under `make -j`, a build script under cargo, and a system-wide jobserver like Gentoo's [steve](https://gitweb.gentoo.org/proj/steve.git) (`MAKEFLAGS=--jobserver-auth=fifo:/dev/steve`).

## Install

```sh
cargo binstall batonpass   # prebuilt binary
cargo install batonpass    # from source
nix profile install github:macalinao/batonpass   # or: nix run github:macalinao/batonpass -- ...
curl -fsSL https://raw.githubusercontent.com/macalinao/batonpass/master/install.sh | sh   # to ~/.local/bin, checksum verified
```

In a NixOS or home-manager flake, add the input `batonpass.url = "github:macalinao/batonpass";` and install `inputs.batonpass.packages.${pkgs.stdenv.hostPlatform.system}.baton`. Prebuilt binaries for Linux (x86_64, aarch64; static musl) and macOS (aarch64, x86_64) are on the [releases](https://github.com/macalinao/batonpass/releases) page.

## Usage

```sh
baton -n 4 -- vitest run --maxWorkers={jobs}  # {jobs} becomes the count it got
baton -n 4 --set RAYON_NUM_THREADS -- ./crunch
baton --serve 12 -- turbo run test            # no jobserver? start one for the tree
```

More:

```sh
baton -n 4 -- oxlint                          # BATON_JOBS=4 in its env
baton -n 8 --min 2 -- tsc -b                  # start with 2 if that's all that's free
baton -n 4 --set UV_THREADPOOL_SIZE -- node build.mjs
baton -n 6 --set GOMAXPROCS -- tsgo
```

In a turbo task (turbo strips `MAKEFLAGS` in strict env mode, so pass it through with `"globalPassThroughEnv": ["MAKEFLAGS", "CARGO_MAKEFLAGS"]` in `turbo.json`):

```json
{ "scripts": { "test": "baton -n 4 -- vitest run --maxWorkers={jobs}" } }
```

Without a jobserver, `--serve` starts one for a whole tree of commands, the way cargo does: with `baton --serve 12 -- turbo run test`, the tasks' batons share 12 tokens.

- `-n N`: jobs to run with, counting baton's implicit token, so it takes N−1 tokens from the pool. Default: nproc.
- `--min M`: wait until M jobs are granted, then take more up to N only if they're free right now. Default: N with `-n` (wait for all), otherwise 1.
- `--set VAR`: also export the count as `VAR`. `BATON_JOBS` is always set.
- `--serve N`: with no jobserver in the env, create one with N tokens and pass it to the command in `MAKEFLAGS`. The command gets N jobs.
- `--require`: exit with an error if there's no jobserver in the env (instead of running with `-n` jobs). Also `BATON_REQUIRE=1`.
- `-v`: print the grant to stderr. Also `BATON_VERBOSE=1`.

If `/dev/steve` exists but no jobserver is advertised, baton warns (even without `-v`): set `MAKEFLAGS=--jobserver-auth=fifo:/dev/steve` to use it.

With no jobserver and no `--serve`, baton runs the command at once with `-n` (or nproc) jobs.

The command runs as a child. baton holds the tokens itself (steve accounts tokens per process), writes back the exact bytes it read (steve's tokens can be NUL), forwards SIGHUP/SIGINT/SIGQUIT/SIGTERM sent to it (not the ones a terminal already sent the whole process group), and exits with the command's code, or dies by the same signal. A signal while baton waits for tokens returns what it holds and exits.

Tokens are permission, not a cap the kernel enforces. A tool that ignores its knob can still use more cores. Tools that speak the jobserver themselves (make, cargo, ninja ≥ 1.13) don't need baton. With steve, `-n` above `--per-process-limit` waits forever; use `--min`.

## Compared with

- **stevie** (steve's client): `stevie cmd` runs a command holding one token, and it only talks to steve. baton takes N tokens from any jobserver and tells the command how many it got.
- **sem** (GNU parallel): a counting semaphore of its own, with one slot per command. It doesn't know about the jobserver that make and cargo use, or how many threads each command starts.
- **[heavy](https://docs.rs/crate/heavy)**: a machine-wide pool of flock slots, one per command, with nesting, timeouts and a CI bypass. It's separate from the jobserver, and a command holds one slot whatever its size. baton draws from the pool that make, cargo and steve already share, and sizes the command to its grant.

## Status

0.1. Unix only; tested on Linux and macOS. MSRV 1.85.

## Development

```sh
nix develop          # rust toolchain, treefmt, git hooks
cargo test           # unit tests + runs baton under real pipe and fifo jobservers
nix flake check      # formatting, hooks, package build with tests
nix fmt
```

Layout: one workspace crate, `crates/batonpass` (binary `baton`): `src/opts.rs` (bpaf args), `src/command.rs` (`{jobs}` and env), `src/main.rs` (tokens, child, signals), `tests/jobserver.rs`. See [CONTRIBUTING.md](CONTRIBUTING.md).

## Releasing

[release-plz](https://release-plz.dev) runs on every push to master (`.github/workflows/release-plz.yml`, only once the repo is public). It opens a release PR that bumps the version and updates `CHANGELOG.md`; merging it publishes to crates.io, tags `vX.Y.Z` and creates the GitHub release, and `release-binaries.yml` attaches the prebuilt archives (`baton-<target>.tar.gz` plus `.sha256`) that `cargo binstall` and `install.sh` download.

crates.io uses trusted publishing (OIDC via `rust-lang/crates-io-auth-action`); there is no `CARGO_REGISTRY_TOKEN` secret. Trusted publishing needs the crate to exist, so **0.1.0 is published by hand once**:

1. `cargo publish -p batonpass`, then on crates.io add a trusted publisher: repo `macalinao/batonpass`, workflow `release-plz.yml`.
2. `git tag v0.1.0 && git push origin v0.1.0`, `gh release create v0.1.0 --generate-notes`, then run `release-binaries.yml` for `v0.1.0` (`gh workflow run release-binaries.yml -f tag=v0.1.0`).

Do step 1 before the repo goes public: from then on each push to master runs the release job, which fails until the trusted publisher exists.

Still to do: make the repo public, then protect master to require CI (branch protection on private repos needs a paid plan). Optionally add a `RELEASE_PLZ_TOKEN` secret (GitHub App token or PAT) so CI runs on release PRs.

## License

Apache-2.0. See [LICENSE](LICENSE).
