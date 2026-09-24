# orb — Research notes

> Gathered 2026-09-23/24 against **Claude Code 2.1.280**, **zellij 0.45.0**, **T3 Code v0.0.42 (commit f5ef0dd)**, rustc 1.98.1, macOS.
> These facts decay. Re-verify anything a milestone depends on (`claude --help`, `claude agents --json`, a real transcript) before building on it.
> Provenance tags: **[verified]** checked on this machine · **[docs]** official docs · **[reported]** from a research pass, not re-checked.

## 1. Claude Code CLI

### Background supervisor
- `claude --bg` / `--background` — start in the background, print a short id. With `--resume <session-id>` it continues that session in the background under the same id (or starts a copy if it's already running). **[verified: `--help`]**
- `claude --bg -n <name>` **with no prompt works** → `backgrounded · <id> · <name> (idle — send a prompt to start)`. **[verified]**
- `--bg` ignores `--session-id` (take the id it prints). **[reported]**
- Subcommands: `agents`, `attach <id>`, `logs <id>` (recent raw terminal output), `stop|kill <id>` (conversation kept), `respawn [id]`/`--all`, `rm <id>` (also removes its worktree when safe). No `send` command exists. **[verified]**
- `claude attach <id>` help: "← returns to agent view, Ctrl+Z drops back to your shell. The session keeps running either way." **[verified]**
- `claude attach` latency: **~200 ms to first byte** in a PTY (2 runs). **[verified]**
- Supervisor data (not a stable interface): `~/.claude/jobs/<id>/state.json`, `timeline.jsonl`, `~/.claude/daemon/roster.json` (worker `decModes` `[1000,1002,1003,1006,2004,1004,2031]`). **[reported]**
- Research preview since v2.1.140. **[reported]**

### `claude agents --json [--all] [--cwd <path>]`
- Official, TTY-free, ~160 ms per call. **[reported]** Lists interactive and background sessions. **[docs]**
- Observed record for an idle, prompt-less `--bg` session **[verified]**:
  ```json
  {"pid": 75288, "id": "3eb0ec4f", "cwd": "/Users/felixpherry/dev/orb", "kind": "background",
   "startedAt": 1790213649662, "sessionId": "3eb0ec4f-a210-4ab8-aa22-6f611c06d753",
   "name": "orb-probe-noprompt", "status": "idle", "state": "blocked"}
  ```
- Field values **[reported/docs]**: `status` ∈ `busy|waiting|idle`; `waitingFor` ∈ `permission prompt|input needed|sandbox request|worker request|dialog open`; `state` ∈ `working|blocked|done|failed|stopped`; `startedAt` in ms.
- Note `state: "blocked"` also appears for an idle session with no prompt — don't read `blocked` alone as "needs you".

### Spawning `claude` / `claude attach` in a PTY
- **Scrub env vars** before spawning (a leaked `CLAUDE_CODE_CHILD_SESSION` disabled transcript saving): `CLAUDECODE`, `CLAUDE_CODE_CHILD_SESSION`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_MESSAGING_SOCKET`, `CLAUDE_CODE_MESSAGING_TOKEN`, `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_PID`. **[reported]**
- Set `TERM=xterm-256color`, `COLORTERM=truecolor`. **[reported]**
- **Startup queries** the emulator must answer: DA1 `CSI c`, XTVERSION `CSI > 0 q`, kitty keyboard `CSI ? u`; on attach also DECRQM 2026 `CSI ? 2026 $ p`. Replies decide sync output + kitty keyboard use. **[reported]**
- Attached sessions always run **fullscreen**: alt screen `?1049h`, mouse `?1000/1002/1003/1006h`, bracketed paste `?2004h`, focus events `?1004h`, `?2031h`. Scrollback lives inside Claude (PgUp/PgDn, `Ctrl+O` transcript). **[reported]**
- Fullscreen redraws only changed cells; hosts that mishandle it garble (zellij issues anthropics/claude-code#42930, #49899). orb re-renders through its own emulator, so it sits between Claude and zellij. **[reported]**
- Env knobs: `CLAUDE_CODE_FORCE_SYNC_OUTPUT=1`, `CLAUDE_CODE_DISABLE_MOUSE=1`, `CLAUDE_CODE_ALT_SCREEN_FULL_REPAINT=1`, `CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN=1`, `CLAUDE_CODE_DISABLE_AGENT_VIEW=1` (for self-hosted PTYs). **[reported/docs]**
- Window title via OSC 0 (`✳ Claude Code`, later the session title). **[reported]**
- Send focus in/out (`CSI I`/`CSI O`); Claude uses them for "away" detection / notifications. **[docs]**
- Workspace-trust dialog appears the first time Claude runs in a new directory. **[reported]**
- Each `claude` process ≈ 300–480 MB RSS. **[reported]**

### Headless mode (rejected, for the record)
- `claude -p --input-format stream-json --output-format stream-json`: `/context`, `/usage`, `/compact`, `/clear`, `/rename` work; `/model`, `/mcp`, `/config` only in text/arg form; `/status`, `/hooks`, `/resume`, `/permissions`, `/memory`, `/plugin` refused ("isn't available in this environment"). **[reported]**
- `--sdk-url` is reserved for Anthropic's Remote Control backend. The messaging socket is peer-to-peer messaging, not a control channel. **[reported]**

### Side channels (backlog)
- Hooks can be injected per launch with `--settings '<json>'` (merges with user hooks). Events: `SessionStart`, `UserPromptSubmit`, `PermissionRequest` (instant; `Notification` of type `permission_prompt` only fires after ~6 s), `Stop` (`last_assistant_message`), `StopFailure`, `SessionEnd` (`reason`), `PreCompact`/`PostCompact`, `SubagentStart`/`SubagentStop`. **[reported/docs]**
- Statusline command receives JSON: `session_id`, `session_name`, `model.*`, `cost.total_cost_usd`, `cost.total_duration_ms`, `context_window.used_percentage`, `rate_limits.*`, `workspace.git_worktree`, `transcript_path`. **[docs]**
- `~/.claude/sessions/<pid>.json` has `status`/`waitingFor`/`statusUpdatedAt` (unofficial). **[reported]**

### Claude's own keys (avoid collisions)
- Option/Alt: `Alt+B/F/D/Y` (word nav/delete/paste-cycle), `Option+P` model, `Option+T` thinking, `Option+O` fast mode, `Alt+M`/`Alt+V` (Windows). `Ctrl+W` delete-back-to-whitespace. `Shift+Tab` cycle permission modes. `Ctrl+O` transcript. `Esc` interrupt / vim NORMAL. `←` on empty prompt → agent view. **[docs: code.claude.com/docs/en/interactive-mode]**
- Vim editor mode: `/config` → Editor mode, or `"editorMode": "vim"` in settings; `vimInsertModeRemaps` e.g. `{"jj": "<Esc>"}`. The user's settings already contain `editorMode`. **[docs/verified]**
- Claude probes kitty graphics (plain `claude`) and has `CLAUDE_CODE_FORCE_TERMINAL_IMAGES`; unanswered probes fall back, so orb's emulator needs no graphics support. **[verified: probe + bundle]**

## 2. Transcript JSONL — `~/.claude/projects/<escaped-cwd>/<sessionId>.jsonl`

**[verified]** on a 300-line real transcript. Append-only, one JSON object per line. Subagents live in `<sessionId>/subagents/agent-<id>.jsonl`. **[reported]**

| `type` | Shape | Preview |
|---|---|---|
| `user` | `message.content` = string, or blocks `text` / `image` / `tool_result {tool_use_id, is_error, content}`; tool-result lines also carry rich `toolUseResult` (Bash: `stdout, stderr, interrupted, isImage, noOutputExpected`) and `sourceToolAssistantUUID` | prompt block, or attach result to its tool call |
| `assistant` | **one content block per line** (`thinking` / `text` / `tool_use {id, name, input, caller}`); lines of one API message share `message.id` (71 lines → 37 ids). `message` has `model`, `stop_reason`, `usage`, … | merge by `message.id` |
| `system` | `subtype`: `turn_duration`, `away_summary`, `local_command`, `api_error`, `informational` | system block (some hidden) |
| `ai-title` | Claude's generated title (`aiTitle`) | thread title |
| `attachment`, `queue-operation`, `last-prompt`, `mode`, `permission-mode`, `cost-state`, `file-history-snapshot`, `atis-latch`, `summary`, `pr-link`, `agent-name` | bookkeeping (96/300 lines were `attachment`) | hidden |

Common line fields: `uuid`, `parentUuid` (tree — rewinds/edits branch it), `sessionId`, `timestamp`, `cwd`, `gitBranch`, `isSidechain`, `version`, `entrypoint`; assistant lines add `requestId`, `effort`.

Path escaping: `/` and `.` → `-` (e.g. `-Users-felixpherry-dev-orb`). Prefer globbing `~/.claude/projects/*/<sessionId>.jsonl` once and storing the path.

## 3. T3 Code (product reference)

Source: https://github.com/pingdotgg/t3code (commit f5ef0dd). Local data: `~/.t3/userdata/state.sqlite` (read-only!).

### Sidebar
- Sections `pinned | active | snoozed | settled` (`apps/web/src/components/Sidebar.logic.ts`). Settled shelf collapsible (10 rows, then pages of 25), sorted by `settled_at` desc. Undo toast 5 s for settle/snooze/archive/unpin.
- Status pill priority: Pending Approval > Awaiting Input > Working (session `running`) > Connecting (`starting`) > Plan Ready > background Working/Monitoring > Completed (unseen: `latestTurn.completedAt > lastVisitedAt`, client-side).
- Elapsed: from `turn.startedAt ?? turn.requestedAt ?? session.updatedAt`; label `42s` / `7m` / `1h 5m`.

### Settle rules (`apps/server/src/orchestration/{ThreadSettlementPolicy,ThreadSettlementReactor,decider,projector}.ts`)
- `settled_override`: `NULL` (neutral, can auto-settle) · `'settled'` · `'active'` (user un-settled; blocks auto-settle until activity resets to `NULL`). `settled_at` = last-activity time for auto-settles. `unsettled_at` = re-entry stamp so an un-settled thread sorts to the top.
- Auto-settle sweep every minute: settle when last activity (latest user message or turn request/start/complete) is older than `sidebarAutoSettleAfterDays` (default 3), or a linked PR merged/closed after the latest user activity (`sidebarAutoSettleOnMerge`, default on). Blocked when archived, override not NULL, pending approval/input, session starting/running, live background work, queued turn start (<2 min), or snoozed without a wake.
- Un-settle on activity (reason `activity` → override NULL): user sends/starts a turn, session goes starting/running, approval or user-input requested. Un-settle by user (reason `user` → override `'active'`): explicit un-settle, or pinning a settled thread.
- Manual settle removes pin + snooze; rejected while running or with a pending approval.

### Drafts
- Client-only (localStorage `composerDraftStore.ts`): `projectId, envMode: local|worktree, branch, worktreePath, runtimeMode, interactionMode, startFromOrigin`. Promoted to a server thread on first send.

### Worktrees & branches
- `git worktree add -b t3code/<8hex> ~/.t3/worktrees/<repo-basename>/<branch with / → -> <base>` (`apps/server/src/vcs/GitVcsDriverCore.ts`). `newWorktreesStartFromOrigin` default true. After the first message an LLM renames the branch; the directory keeps its name.
- Previous-worktree seed: most recently updated non-archived thread in the project with a different `worktreePath` (`resolvePreviousWorktreeSeed`).
- Branch switch: if the branch is checked out in some worktree, reuse it; else checkout in the thread's cwd; server stops the provider session if cwd changed.

### Why T3 can't do native commands
- Uses `@anthropic-ai/claude-agent-sdk` `query()` (`apps/server/src/provider/Layers/ClaudeAdapter.ts`). `/status` isn't exposed by the SDK; `/context` runs but its `local_command_output` is dropped (`ClaudeAdapter.ts:~3951`). Issue #1329.

### Projects (import source)
- `projection_projects(project_id, title, workspace_root, …, deleted_at)`. Import title + workspace_root where `deleted_at IS NULL`.

## 4. jinn reference map (`~/dev/jinn`, paths as of 2026-09-23 — grep if moved)

| Concern | Where |
|---|---|
| Style guide / architecture rules | `AGENTS.md`, `.agents/RECORD.md` |
| Wiring (runtime, actors, supervisor) | `src/main.rs`, `src/app.rs`, `src/actor_wiring.rs`, `src/runner.rs` |
| Intent enum / handler | `crates/jinn-domain/src/protocol/intent.rs`, `crates/jinn-domain/src/feat/intent/handler.rs` |
| AppState / State / capability tokens | `crates/jinn-domain/src/common/{app_state,state}.rs`, `common/tcaps/*.rs` (`mint.rs`) |
| Bus / bridge / actor deps | `common/bus.rs` (`BusMessage`, kameo `MessageBus`), `common/bridge.rs`, `common/actor_deps.rs`; example actor `feat/quake_bar/quake_bar_actor.rs` |
| Render loop | `crates/jinn-tui/src/run.rs`, `msg/handler.rs` (100 ms tick, 33 ms throttle — orb should wake on events instead) |
| Keymap / scopes / which-key | `crates/jinn-tui/src/keymap.rs` (`ratatui-which-key` 0.14 builder), `crates/jinn-tui/src/scope.rs`, `crates/jinn-domain/src/common/focus.rs` (`FocusScope`, `ScopeStack`) |
| Keys / kitty enhancement | `crates/jinn-domain/src/protocol/key.rs`, `crates/jinn-tui/src/convert.rs` (no BackTab!), `crates/jinn-tui/src/terminal.rs` |
| Picker | `crates/jinn-selection-widget` (`PickerItem`, `SelectionState<T>`, SkimMatcherV2) |
| Sidebar | `crates/jinn-domain/src/feat/ui/sidebar/` (`section_trait.rs`, `sessions/render/entry_line.rs`) |
| Chat log blocks | `crates/jinn-domain/src/feat/ui/chat_log/` (`visual_item.rs`, `line_count_cache.rs`, `markdown.rs`, `tool_call.rs`, `tool_result.rs`, `thinking.rs`, `user.rs`, `assistant.rs`) |
| Markdown | `ratatui-markdown` (crates.io, MIT/Apache; jinn vendors a patched copy in `vendor/`) |
| PTY / emulator | `crates/jinn-domain/src/feat/interactive_term/` (`pty_session.rs`, `screen_task.rs`, `emulator.rs`, `query_responder.rs`, `settle.rs::encode_key_event`), `crates/jinn-tui/src/render/terminal_tab.rs` — **known gaps**: legacy-only key encoding (Shift+Enter lost), no BackTab, paste/mouse not forwarded, ~150 ms output latency |
| External editor suspend | `crates/jinn-tui/src/suspend.rs` |
| SQLite migrations / DAO | `crates/jinn-session-schema` (`migrate.rs`), `crates/jinn-domain/build.rs` (daow compile-time SQL check), `feat/session/session_store/sqlite.rs` |
| Test helpers | `crates/jinn-testutil` (`TestBackend` helpers), `.cargo/config.toml` (`RSTEST_TIMEOUT`), `justfile` `lint-testattr` |

## 5. User environment

- kitty on macOS, **no** `macos_option_as_alt` (Option types symbols). `kitty.conf` maps `cmd+{h,i,j,k,l,n,o,p,x,1-5,[,],f,+,-,=,arrows}` → `super+…` for zellij.
- zellij 0.45.0 (`~/.config/zellij/config.kdl`): `keybinds clear-defaults=true`, `default_mode "locked"`, `Ctrl g` toggles lock, navigation on `Super`. Kitty keyboard protocol on (default). Kitty graphics supported since 0.45 (https://zellij.dev/news/nested-sessions-kitty-graphics-new-ui/).
- zellij CLI used by M8: `zellij action new-pane [--floating] [--name] [--cwd] [--width/--height] -- <cmd>` (returns pane id), `zellij action list-panes --json [-a]`, `zellij action focus-pane-id <id>`, `go-to-tab-by-id`. **[verified: `--help`]**
- Tools: nvim, lazygit, yazi, gh installed; `$EDITOR` unset in non-interactive shells (fall back to `nvim`).
- Related tool the user runs: `cwt` (worktree manager TUI) — opens duplicate zellij panes on re-entry; orb must de-dupe.

## 6. Terminal pane (verified 2026-09-24, Claude Code 2.1.281, alacritty_terminal 0.26.0)

Tags: **probe** = PTY byte capture on this machine · **bundle** = read from Claude's JS bundle · **source** = read from the crate source in `~/.cargo/registry`.

### `claude attach`
- It sends only `CSI > 0 q` + DA1 `CSI c` after the first frame. Sometimes a second round follows: `CSI ? 2026 $ p` + DA1. **[verified: probe]**
- It never blocks on replies, and replies don't change its behaviour. Caps are computed from the **attach client's env** at connect. **[verified: probe]**
- Kitty keys are on iff the terminal name is in `iTerm.app, kitty, WezTerm, ghostty, tmux, windows-terminal, WarpTerminal`. The name comes from `TERM_PROGRAM`, else `TERM` (`xterm-kitty` → kitty, `xterm-ghostty` → ghostty), else `KITTY_WINDOW_ID` → kitty. **[verified: bundle + probe]**
  - When on it sends `CSI < u`, `CSI > 5 u` (`> 1 u` when session/client versions differ), and `CSI > 4 ; 2 m`. **[verified: probe]**
  - There is no kitty-specific env var. **[verified: bundle]**
- `CLAUDE_CODE_FORCE_SYNC_OUTPUT=1` wraps every attach frame in `?2026h/l` **[verified: probe]**; it is ignored under `TMUX` **[verified: bundle]**.
- DECSTBM scroll regions are used only with sync on and none of `TMUX`, `ZELLIJ`, JetBrains, xterm.js, `WT_SESSION`. **[verified: bundle]**
- Attach mode sets: `?1049h ?1000h ?1002h ?1003h ?1006h ?2004h ?2031h ?1004h`. **[verified: probe]**
- `Ctrl+Z` (0x1a) exits attach with code 0 in ~0.5 s; the session keeps running. **[verified: probe]**
- Mouse knobs: `CLAUDE_CODE_DISABLE_MOUSE`, `CLAUDE_CODE_DISABLE_MOUSE_CLICKS` (keeps scroll). **[verified: bundle]**

### Plain `claude`
- Round 1: `CSI > 0 q`, `CSI ? u`, DA1. **[verified: probe]**
- Round 2 runs only if XTVERSION was answered and `TERM_PROGRAM` isn't `Apple_Terminal`: `CSI ? 2026 $ p`, a kitty graphics query (`ESC _ G i=31,s=1,v=1,a=q,t=d,f=24;AAAA ESC \`), `CSI 16 t`, `CSI ? 1016 $ p`, DA1. **[verified: probe + bundle]**
- Each round waits up to 2000 ms for the DA1 sentinel. **[verified: bundle]**
- It sends the kitty push after a `CSI ? 0 u` reply. **[verified: probe]**

### Environment
- Extra session env vars seen in a Claude-spawned shell: `CLAUDE_CODE_EXECPATH`, `CLAUDE_CODE_SESSION_ATTENDED`, `CLAUDE_EFFORT`, `CLAUDE_AGENT_SDK_VERSION`. **[verified]** The bundle also reads `CLAUDE_CODE_SESSION_KIND` (`bg` = worker). **[verified: bundle]**
- `claude --bg` refuses in an untrusted dir: "Workspace not trusted. Run `claude` in … once and accept the trust prompt". **[verified]**
- `agents --json` records: stopped/done background records have no `pid`/`status`; interactive records have no `id` and do have `waitingFor`. **[verified]**

### Libraries
- **alacritty_terminal 0.26** **[verified: source + probe]**:
  - doesn't parse XTVERSION
  - answers `CSI ? u` only with `Config.kitty_keyboard = true`
  - answers DECRQM 2026 with `;2`; doesn't handle `?2031`
  - its sync timeout must be fired by the caller (`Processor::sync_timeout().sync_timeout()` + `stop_sync`)
  - `ColorRequest`/`TextAreaSizeRequest` need caller replies
  - `tty::new` can't remove env vars and exits the process on a failed resize ioctl
- **portable-pty 0.9:** cwd defaults to `$HOME`; `take_writer` works once; dropping the writer sends `\n` + ^D to the child. **[verified: source]**
- **terminput 0.5.15:** kitty mode encodes unmodified Enter/Tab/Backspace as CSI u; legacy mode ignores DECCKM; its `KittyFlags` bits are shifted. **[verified: source + probe]**
- **crossterm 0.29:** legacy `0x1C` parses as Ctrl+`4` (`src/event/sys/unix/parse.rs:110`). **[verified: source]**
