# Contributing

Bug reports, ideas and pull requests are welcome.

## Reporting a bug

Open an issue and include:

* your distro and desktop environment, and how you installed (AppImage, deb, rpm, source);
* the output of `gdrive status` and `gdrive --version`;
* the relevant daemon log: `journalctl --user -u gdrived --since "10 min ago"`.

Never paste your `token.json` or OAuth client secret.

## Development setup

See "Build and install (from source)" in the [README](README.md). In short:

```sh
npm --prefix app ci && npm --prefix app run build   # frontend, embedded by the app
cargo test --workspace                              # engine, daemon, CLI tests
cd app && cargo tauri dev                           # UI with hot reload
```

Layout: `crates/gdrive-core` (sync engine), `crates/gdrived` (daemon),
`crates/gdrive-cli` (CLI), `app/src-tauri` (tray + window commands), `app/src` (React UI).

## Pull requests

* Keep a change focused; one fix or feature per PR.
* Add or update tests for sync-engine changes. Anything that could lose or
  overwrite user data deserves a test.
* Run `cargo fmt` and `cargo clippy --workspace` before pushing.
* Describe what changed and why; link the issue if there is one.

Good places to start: issues labelled
[good first issue](../../labels/good%20first%20issue).

By contributing you agree your work is released under the [MIT license](LICENSE).
