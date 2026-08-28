# otter-ai-cli

Unofficial command-line tool for [Otter.ai](https://otter.ai). It is not affiliated with, endorsed by, or part of Otter.ai.

For people on an [Otter Pro](https://otter.ai/pricing) plan (or another paid Otter plan) who want a simple way for humans and agents to access their own conversations from the terminal.

## Install

Anyone can install this. You need a [Rust toolchain](https://rustup.rs):

```bash
cargo install --git https://github.com/andrewfurman/otter-ai-cli
```

That puts an `otter` binary on your `PATH`.

## Use

Log in with your own Otter account, then list conversations:

```bash
otter login
otter speeches list --days 2
```

Most commands take `--json` for scripts and agents. `otter --help` is the full command surface. Notes for coding agents are in [AGENTS.md](AGENTS.md).

CLI speech IDs are Otter **otid** values (from `otter speeches list`), not the internal `speech_id`.

Use this only with an Otter account you are allowed to access, and follow [Otter.ai's Terms of Service](https://otter.ai/terms-of-service).

## License

MIT. Originally based on [gmchad/otterai-api](https://github.com/gmchad/otterai-api). Forks are welcome. All changes to this repository should be done through a pull request; anyone is free to make a pull request.
