# otter-ai-cli

Unofficial command-line tool for [Otter.ai](https://otter.ai). It is not affiliated with, endorsed by, or part of Otter.ai.

For people on an [Otter Pro](https://otter.ai/pricing) plan (or another paid Otter plan) who want a simple way for humans and agents to access their own conversations from the terminal.

Single Rust crate: command modules under `src/` are the CLI, `src/client.rs` is the unofficial HTTP client, and `src/config.rs` stores credentials in `~/.otterai/config.json` (`OTTERAI_USERNAME` / `OTTERAI_PASSWORD` override the file).

## Install

Anyone can install this. You need a [Rust toolchain](https://rustup.rs):

```bash
cargo install --git https://github.com/andrewfurman/otter-ai-cli
```

That puts an `otter` binary on your `PATH`. From a checkout: `cargo install --path .`

## Use

Log in with your own Otter account, then list conversations:

```bash
otter login
otter speeches list --days 2
```

Most commands take `--json` for scripts and agents. `otter --help` (and `otter <group> --help`) is the full command surface.

CLI speech IDs are Otter **otid** values (from `otter speeches list`), not the internal `speech_id`.

Use this only with an Otter account you are allowed to access, and follow [Otter.ai's Terms of Service](https://otter.ai/terms-of-service).

## Develop

```bash
cargo build
cargo test                  # live API tests skip unless OTTERAI_USERNAME/OTTERAI_PASSWORD are set
cargo fmt --all
cargo clippy --all-targets
```

When running live mutation tests, upload a throwaway file and trash it afterward.

The API is unofficial and drifts. Response JSON stays untyped on purpose. `finish_speech_upload` needs `appid=otter-web`. The CLI logs in on every command, so a burst of invocations can hit a login rate limit (HTTP 429). Batch where you can (`otter speeches move ID1 ID2 ID3 --folder FOLDER`) and wait about 60–90 seconds on 429.

## License

MIT. Originally based on [gmchad/otterai-api](https://github.com/gmchad/otterai-api); the last Python tree is the `python-final` tag. Keep the LICENSE file. Forks are welcome. All changes to this repository should be done through a pull request; anyone is free to make a pull request.
