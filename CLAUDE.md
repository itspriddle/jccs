# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`jccs` is a Rust CLI that Claude Code runs as its statusline command. It reads Claude Code's statusline JSON on stdin and prints one or more styled lines, as configured in `~/.config/jccs/config.toml` (or `$XDG_CONFIG_HOME/jccs/config.toml`). It also has the subcommands `vars`, `configure`, `install`, and `uninstall`.

## Commands

```sh
make check                    # lint + test (what CI enforces)
make lint                     # cargo fmt, then clippy --all-targets --locked -D warnings
make test                     # cargo test --locked
make vuln                     # cargo deny check (advisories/licenses/sources)
cargo test <name>             # run one test, e.g. cargo test template_matches_defaults
cargo test config::tests::    # run one module's tests

# Try it by hand
cargo run -- vars < docs/schema.json            # list every value
cargo run -- --format '{model.family}' < docs/schema.json
cargo run -- --config ./test.toml --color-mode none < docs/schema.json
```

CI runs `cargo fmt --all --check`, clippy with `-D warnings`, tests on Ubuntu and macOS, and cargo-deny. Clippy `pedantic`, `unwrap_used`, and `expect_used` are warnings, so with `-D warnings` they fail the build. Don't use `unwrap()` or `expect()` outside tests. `unsafe` is forbidden. The crate uses edition 2024 with MSRV 1.94.

## Architecture

The render pipeline in `main.rs`:

1. `input.rs`: deserializes stdin into `StatusInput`. Every field is optional and unknown fields are ignored, so changes to Claude Code's payload never break parsing. `docs/schema.json` is a sample payload, and a test parses it.
2. `status.rs`: builds a `Status` of derived, display-ready values from the input plus `Env`, which holds the side-effectful inputs: the current time, the git branch from `git.rs`, and today's cost from `ledger.rs`. Keeping these in `Env` keeps `Status::from_input` pure and testable.
3. `vars.rs`: flattens `Status` into ordered `(name, Value)` pairs. Missing values are omitted. **`NAMES` is the registry of every value name.**
4. `config.rs`: loads the TOML (`deny_unknown_fields`). `Config::compile()` checks the templates, icons, and colors and produces a `Layout`. `Layout::render(&vars)` joins segments with the separator. A segment is dropped if any value it references is missing or if its `when` value is falsy. `segments` is either one line (a flat array) or several lines (an array of arrays).
5. `template.rs`: the segment markup: `{value}`, `{icon.x}`, `[style]…[/]`, `[$value]` for a color chosen by a value, and `{{`/`[[` escapes. Value names are checked against `vars::NAMES` when the template is parsed.
6. `style.rs`: colors (built-ins, `#rrggbb`, 0–255), modifiers, SGR output, and `ColorMode` (truecolor / 256 / none). `NO_COLOR` and `--color-mode` override the config.

Other modules:
- `ledger.rs`: tracks today's spend across all sessions with per-session files in the state dir. Files store absolute values, not running sums, and are written atomically, so concurrent renders can't double count. Session IDs are validated (`is_safe_id`) before they're used in paths.
- `install.rs`: edits Claude Code's `settings.json` to add or remove the `statusLine` entry. It plans a list of `Action`s, prints them, asks for confirmation (`--yes` skips this), and then applies them.

Errors are printed to **stdout**, not stderr, as `jccs: …`. Claude Code only displays stdout, so this is deliberate.

## Invariants enforced by tests

When adding or renaming a value, keep all of these in sync or tests fail:
- `vars::NAMES` must match the exact order in which `Status::vars()` pushes values.
- Every name in `NAMES` must be documented in `config.example.toml`.
- `config.example.toml` is embedded with `include_str!` as the `jccs configure` template. It must render identically to `Config::default()` (`template_matches_defaults`). If you change a default in `config.rs` (`DEFAULT_SEGMENTS`, `DEFAULT_ICONS`, `DEFAULT_COLORS`, `ContextConfig`), update the example file too.
- If you add an input field, add it to `docs/schema.json` as well.

## Conventions

- Commits follow Conventional Commits (`feat:`, `fix:`, `chore:`, `feat!:` for breaking changes), and branches follow Conventional Branch (`feature/…`, `claude/…`). Project skills for both are in `.claude/skills/`.
- Releases are cut by pushing a `v*` tag. `.github/workflows/release.yml` builds for macOS arm64 and Linux musl (x86_64/arm64), then updates the Homebrew tap.
