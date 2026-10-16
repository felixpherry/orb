# orb — guide for coding agents

orb is a terminal multiplexer for coding agents (Claude Code and pi), written in Rust. It runs inside kitty, keeps every pane alive under zmx, and shows what each agent is doing in a sidebar.

Read this file before any change. Open a topic file below only when your change touches its area.

## Commands

Run everything through `just`. Skills name commands by role; here is what each role runs:

- **check** — `just check`.
- **test** — `just test`, the whole workspace. Never run `cargo test -p <crate>`.
- **lint** — `just lint`: clippy with warnings as errors, the fmt check, the test-attribute guard, the doc and safety rules in scripts/lint-rules.py, and the file-length cap.
- **format** — `just fmt-fix`.
- **commit** — `just commit '<message>'`. It stages every change, so look over what is staged first.
- **sync-trunk** — `git merge main`, then fix conflicts and test again.
- **vcs** — git. `main` is the trunk. Don't push, and don't merge into `main`, unless the user asks.

`just --list` shows the rest, such as `just coverage` (an lcov file for the workspace) and `just debt` (a debt report weighted by coverage).

Commit only when `just test` passes. A branch is done only when `just lint` passes too.

## Where things live

- crates/orb-domain — app state, intents, commands, actors, and one folder per feature under src/feat/.
- crates/orb-tui — the screen loop, the keymap, and everything drawn.
- crates/orb-term — the terminal emulator behind each pane.
- src/main.rs — start-up: reads the environment, builds the services, spawns the actors, runs the loop.

## Rules for every change

- **Errors are reports.** Fallible code returns `error_stack::Report` over a `wherror` type that sits in the same file as the code that fails. No error.rs or traits.rs files.
- **Outside tools sit behind a trait.** git, zmx, the harnesses and the notifiers each have one, with a fake for tests.
- **Each state field has one writing actor.** Only the actor that owns an `AppState` field writes it; the intent handler is the one exception.
- **Outside programs get a time limit.** Run them through `run_within`, or wait with `wait_within`. The lint rejects a bare `.output()`, a `.status()` in production code, and a bare `.wait()`.
- **Never drop the result of a kill or a removal** with `let _ =`.
- **Cut text by grapheme** with the unicode-segmentation crate, never with `.chars()` or byte indexes.
- **Read environment variables only at start-up** in src/main.rs, then pass the values along.
- **Tests check behaviour you can see.** One behaviour per test, Given/When/Then comments, `#[rstest::rstest]` on every test, a message on every assert.
- **Bind only the keys the user asked for.** Direct keys use Alt. Never Super.
- **Code comments describe the code.** No notes about plans, specs or divergences; plans aren't kept.
- **Keep files under 2,000 production lines.** Split one by concern before it passes that.

## Topic files

| File | Read it when you |
| --- | --- |
| docs/dev/architecture.md | add an intent, command, actor, report or state field, or touch the screen loop |
| docs/dev/rust-style.md | write Rust: errors, traits, modules, actors, docs, the small style rules |
| docs/dev/testing.md | write or change a test |
| docs/dev/keybindings.md | add or change a key |

<!-- private -->
## Private docs

These live only in the private repo.

- docs/product.md — why orb exists and how it behaves: the UX spec, the key list (its Keys section), storage, the backlog.
- docs/decisions.md — choices made and the options turned down. Leave a decision closed unless the user reopens it.
- docs/roadmap.md — the architecture overhaul: the debt found, the target, the steps, the drift log.
- docs/research.md — outside facts that were checked: versions, CLI flags, file formats. Check a version-bound fact again before you rely on it.
- .agents/RECORD.md — how the app works today. **When a feature or an overhaul step is done, add its new facts here**, each one checked first.
- .plans/<task>/ — task plans, git-ignored. plan.md is the spec: annotate it, never rewrite it. tasks.md tracks progress. phase-N.md holds each phase's plan and review.
- On the user's Mac, Option belongs to paneru, so the user reaches orb's Alt keys with Cmd through kitty's `map cmd+X send_key alt+X`.
<!-- /private -->
