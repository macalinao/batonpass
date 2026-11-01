# Contributing

Issues and PRs are welcome. For anything larger than a fix, open an issue first.

```sh
nix develop      # toolchain, treefmt, git hooks (or use rustup with Rust 1.85+)
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
nix fmt          # or cargo fmt
```

CI also runs `cargo deny check`, the MSRV build (Rust 1.85) and the tests on macOS. Use [Conventional Commits](https://www.conventionalcommits.org) (`fix:`, `feat:`) in PR titles: release-plz builds the changelog from them. PRs are squash-merged.

By contributing you agree your work is licensed under Apache-2.0.
