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

Path escaping: every non-alphanumeric char → `-` (e.g. `-Users-felixpherry-dev-orb`; see §7). Prefer globbing `~/.claude/projects/*/<sessionId>.jsonl` once and storing the path.

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
| Markdown | `ratatui-markdown` (crates.io, MIT/Apache; jinn vendors a patched copy in `vendor/`). orb uses `tui-markdown` 0.3.9 instead (§8). |
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

## 7. Sessions (verified 2026-09-24, Claude Code 2.1.281)

Tags as in §6; **source** for libraries = read from the crate source in `~/.cargo/registry`.

### `claude --bg`
- In a trusted dir it prints `Starting background service…`, then `backgrounded · <id> · <name> (idle — send a prompt to start)` with `-n <name>`, or `backgrounded · <id> (idle — send a prompt to start)` without. Takes ~1.2 s. Without `-n`, `name` is the short id. **[verified]**
- In an untrusted dir it refuses with "Workspace not trusted. Run `claude` in … once and accept the trust prompt" (§6). **[verified]**

### `claude agents --json --all`
- ~175 ms wall, ~120 ms CPU per call — polling every 1 s costs ~12% of a core. **[verified]**
- Record shapes (running background, stopped background, interactive):
  ```json
  {"pid":960,"id":"28bf38e2","cwd":"/Users/felixpherry/dev/orb","kind":"background","startedAt":1790233098717,
   "sessionId":"28bf38e2-8929-4841-b907-f87d5d469a10","name":"orb-m2-probe","status":"idle","state":"blocked"}
  {"id":"28bf38e2","cwd":"…","kind":"background","startedAt":1790233097892,"sessionId":"…","name":"orb-m2-probe","state":"stopped"}
  {"pid":63163,"kind":"interactive","startedAt":1790143927372,"name":"itemku-frontend-next-v2-18","status":"waiting","waitingFor":"dialog open"}
  ```
  **[verified]**
- A prompted turn shows `busy`/`working` within ~1 s of Enter, then `idle`/`done`. **[verified]**
- After `claude rm <id>` the record is gone. **[verified]**
- `startedAt` is the session start, not the turn start. Interactive records have no `id`. **[verified]**

### Transcripts
- A prompt typed through `claude attach` into an idle session leaves `name` as the short id (watched ≥15 s). The transcript gets the `user` line and `ai-title` lines. **[verified]**
- A prompt-less session has **no transcript file**. **[verified]**
- Path: `<claude_dir>/projects/<escaped cwd>/<sessionId>.jsonl`. Every non-alphanumeric ASCII char of the cwd becomes `-` (18/18 existing dirs, e.g. `/Users/felixpherry/.t3/worktrees/x` → `-Users-felixpherry--t3-worktrees-x`). `claude_dir` is `$CLAUDE_CONFIG_DIR`, else `~/.claude`. **[verified]**
- Line shapes relevant to titles (40 real transcripts) **[verified]**:
  - Prompts: `{"type":"user","message":{"content":"<string>"}}`, or `content: [{"type":"text","text":…}, {"type":"image",…}]`.
  - Tool results: `content: [{"type":"tool_result",…}]`.
  - `isMeta: true` lines carry skill bodies / image metadata.
  - Non-prompt user strings start with `<command-message>`, `<command-name>`, `<local-command-…>`, `<bash-input>`, `<bash-stdout>`, or `<task-notification>`, or are `[Request interrupted by user]`.
- `{"type":"ai-title","aiTitle":"…"}` appears several times per transcript; the first may be prompt-derived, the latest is the generated title. **[verified]**

### Env
- `CLAUDE_CODE_DISABLE_AGENT_VIEW=1` / the `disableAgentView` setting disables `claude agents`, `--bg`, and the daemon — never set it. **[verified: bundle]**

### T3 sources (commit f5ef0dd)
- Title: the truncated first message at send (`ChatView.tsx:8289–8305`), replaced by the generated title (`threadTitles.ts` `canReplaceThreadTitle`, `ProviderCommandReactor.ts:971`).
- Elapsed: the adapter stamps `turn.startedAt` when it emits `turn.started` (`ClaudeAdapter.ts:3402`); the sidebar counts from `turn.startedAt ?? turn.requestedAt ?? session.updatedAt` (`Sidebar.logic.ts:992`).

### Libraries
- **kameo 0.22.2** **[verified: source]**:
  - `Actor::spawn` uses a **bounded mailbox of 64**; `tell(..).try_send()` fails when it's full. `spawn_with_mailbox(args, mailbox::unbounded())` avoids that.
  - `tell(..).try_send()` is sync (`Result<(), SendError<M>>`); `ask(..).await` returns the reply.
  - `spawn*` needs a tokio runtime context (`Runtime::enter()` guard).
- **ratatui-which-key 0.14.0** **[verified: source]**:
  - Built on ratatui 0.30 / crossterm 0.29 (orb's versions); LGPL-3.0; jinn uses it.
  - Leader is Space by default; key strings `<c-x>`, `<enter>`, `<esc>`, `<leader>`, plain chars.
  - `WhichKeyState::handle_key` matches with crossterm `KeyEvent ==`, which compares `code, modifiers, kind, state`. Kitty repeats (`KeyEventKind::Repeat`) or `state` bits never match a binding, so feed `KeyEvent::new(key.code, key.modifiers)`.
- **rusqlite 0.40** with `default-features = false` links the system SQLite (as jinn does). **[verified: source]**

## 8. Transcript preview (verified 2026-09-25, Claude Code 2.1.282)

Sample: 207 main transcripts and 67 subagent files under `~/.claude/projects` on this machine. Tags as in §6.

### Transcript lines
- **Which lines carry a uuid.** Only `user`, `assistant`, `attachment`, and `system` lines have `uuid`/`parentUuid`. `ai-title`, `custom-title`, `last-prompt`, `mode`, `permission-mode`, `file-history-snapshot`, `file-history-delta`, `queue-operation`, `atis-latch`, `cost-state`, `agent-name`, `pr-link`, `worktree-state`, `relocated`, and `summary` have neither. **[verified]**
- **Parent links.** **[verified]**
  - `parentUuid` points at the previous uuid line in 10396/10466 cases.
  - Parallel tool results point at their own `tool_use` line (`parentUuid == sourceToolAssistantUUID`, 1698/1698), so a plain `parentUuid` walk drops calls (16/44 sampled files). Example: `32 tool_use A → 33 tool_use B (p=32) → 34 result B (p=33) → 35 result A (p=32) → 36 attachment (p=35)`.
  - One file has a parent that isn't in the file (a naive walk loses 73 lines). One file re-appends 563 lines with already-used uuids (the copies are identical or differ only in `promptId`/`gitBranch`).
- **Forks.** Real forks are rare (4/207 files), e.g. the same prompt sent twice, or a second prompt under the same parent. **[verified]**
- **Newest leaf.** It is the last line with a uuid. `last-prompt.leafUuid` lags 1–9 lines in 6/44 files. **[verified]**
- **Assistant lines.** **[verified]**
  - Each line holds one content block: `thinking` (keys `type, thinking, signature`), `text`, or `tool_use {id, name, input}`.
  - Lines of one API message share `message.id`, and only tool-result user lines appear between them.
  - `model: "<synthetic>"` lines are either API errors (`isApiErrorMessage: true`, e.g. `"API Error: Connection lost mid-response…"`) or `"No response requested."`.
  - 1273 of 1367 thinking blocks are `"thinking":""`.
- **User lines.** `message.content` is a string or a list of `text` / `image` / `tool_result` blocks. A `tool_result.content` is a string or a list of `text` / `image` / `tool_reference` blocks, and `is_error` may be missing. **[verified]**
- **`toolUseResult`.** **[verified]**
  - Edit: `structuredPatch: [{oldStart, oldLines, newStart, newLines, lines: [" ctx", "-old", "+new"]}]`.
  - Write: `{type: "create" | "update", filePath, content, structuredPatch}`; a create has `structuredPatch: []`.
  - On errors it's the string `"Error: …"`.
- **Non-prompt user text.** **[verified]**
  - `isMeta: true` lines.
  - `<command-message>…<command-name>/x</command-name>…<command-args>…</command-args>`.
  - `<local-command-caveat>`, `<local-command-stdout>`, `<task-notification>`, `<bash-input>`, `<bash-stdout>`/`<bash-stderr>`.
  - `[Request interrupted by user]` and `[Request interrupted by user for tool use]`.
- **Queued prompts.** Prompts typed mid-turn are `attachment` lines with `attachment: {type: "queued_command", commandMode: "prompt" | "task-notification", prompt: <string | blocks>}`, not `user` lines. **[verified]**
- **System subtypes seen.** `turn_duration`, `away_summary`, `local_command` (`content` = `<local-command-stdout>…`), `informational` (`content`, `level`), `api_error` (`level: "error"`, `error{message,…}`, `retryAttempt`, `maxRetries`). The binary knows 50+ subtypes. **[verified: data + bundle]**
- **Compaction.** `{type: "system", subtype: "compact_boundary", content: "Conversation compacted", parentUuid: null, logicalParentUuid: <uuid>}`, followed by a user line with `isCompactSummary: true`. No compacted transcript exists on this machine. **[verified: bundle only]**
- **No sidechain lines** appear in main transcripts. **[verified]**
- **Sizes.** Median 653 KB / 179 lines; max 20.2 MB / 1957 lines; longest line 1.69 MB (base64 images). No file ended without a newline. **[verified]**

### Titles
- `/rename` writes `{"type":"custom-title","customTitle":"…","sessionId":"…"}` (no uuid). It is re-appended: the renamed transcript b2fe33f2 has 7 `custom-title` lines (`orb-m1`) and no `ai-title` lines. Because `ai-title` lines keep coming in other transcripts, a custom title only wins if it's kept apart from the `ai-title`/prompt title. **[verified]**

### Libraries
- **`ratatui-markdown`** requires ratatui `^0.29` in every release (checked through 0.3.6), so it can't share types with orb's ratatui 0.30.2. **[verified: source]**
- **`tui-markdown` 0.3.9** (joshka) **[verified: source + scratch build]**:
  - Builds on `ratatui-core` 0.1 (default features off), which is ratatui 0.30's core; with ratatui 0.30.2, `cargo tree -i ratatui-core` shows one version (0.1.2).
  - Pulls `syntect` 5.3 with default features (builds the oniguruma C library), `pulldown-cmark` 0.13, and `ansi-to-tui` 8.
  - `tui_markdown::from_str(&'a str) -> ratatui_core::text::Text<'a>` borrows its input; caching needs an owned copy (`Span` content `.into_owned()`).
  - Output keeps `# ` heading markers and ```` ```lang ```` fence lines, visible and styled.
  - Release timing: the first call ~7.5 ms (syntect loads its syntax set lazily); later calls ~0.8 ms for a 260-line document with 20 code blocks.
- **ratatui-which-key 0.14** parses `"G"` as `KeyEvent::new(KeyCode::Char('G'), KeyModifiers::empty())`, while kitty/crossterm report Shift+g as `Char('G')` + `SHIFT`; drop SHIFT for chars before matching. `"gg"`, `"za"`, `"<tab>"`, `"<c-d>"`, `"<c-u>"` are supported sequences and names. **[verified: source]**
- **ratatui-core** `Buffer::set_stringn` drops graphemes containing control characters, so `\t` and ESC vanish; replace tabs with spaces first. **[verified: source]**
- **ratatui 0.30** `Paragraph::line_count(width)` needs the `unstable-rendered-line-info` feature. **[verified: source]**
- **serde_json** `Map` is a sorted `BTreeMap` unless the `preserve_order` feature is on (then `indexmap`; 2.14 is already in orb's `Cargo.lock`). **[verified: source]**

## 9. Settle lifecycle (verified 2026-09-25, Claude Code 2.1.282; T3 f5ef0dd)

T3 facts are read from the T3 Code source at commit `f5ef0ddb90a8c36584e181b1913e7b8a5df30ffc` (paths relative to the repo). CLI facts were checked on this machine with two probe sessions, removed afterwards. Tags as in §6.

### T3 sidebar layout (`apps/web/src/components/Sidebar.tsx`) **[verified: source]**
- Sections span all projects; each row names its project. Render order: pinned, active, (snoozed), then the settled header and its rows.
- "Pinned"/"Active" labels are zero-height except while dragging; only the settled shelf has a visible header.
- The settled header reads `Settled (N)` collapsed and `Settled` expanded, and starts collapsed. While collapsed, the open thread's settled row still renders.
- Card: favicon/monogram + project name + status slot (the status pill, else a relative time), then the title, then the branch + provider icon.
- Slim settled row: dimmed favicon + title + settled-time label.
- A card's time label is the time since `latestUserMessageAt ?? updatedAt` (`threadTimeLabel`); a settled row's is the time since its settled timestamp (`settledTimeLabel`).

### T3 ordering (`packages/client-runtime/src/state/threadSort.ts`) **[verified: source]**
- Active: threads without an `activeOrderKey` come first, by `max(createdAt, unsettledAt)` desc (`activeThreadAnchorTimestampMs`), so an un-settled thread re-enters at the top.
- Pinned: keyed threads by `pinOrderKey`, then keyless threads by `createdAt` desc; ties by id.
- Settled: by the settled timestamp desc (`resolveSettledThreadTimestamp`).
- `activeOrderKey`/`pinOrderKey` are fractional keys written only by drag reordering.

### T3 settle rules (`apps/server/src/orchestration/{ThreadSettlementPolicy,decider,projector}.ts`) **[verified: source]**
- **Manual settle:** rejected while running or waiting. Writes override `settled`, `settled_at = now`, `unsettled_at = null`, and unpins (and unsnoozes).
- **User un-settle:** override `active`, `settled_at = null`; `unsettled_at = now`, kept if the override was already `active`.
- **Activity** (a user turn, the session starting/running, an approval or input request): only when the override is non-null — override → null, `settled_at = null`; `unsettled_at = now` if it was settled, kept if `active`.
- **Pin:** `pinned_at = now`, kept if already pinned. On a settled thread it also un-settles with reason user.
- **Auto-settle:** when the last activity is older than 3 days, the override is null, nothing is running or pending. Writes `settled_at` = the last-activity time. The policy never checks `pinnedAt`, so T3 auto-settles (and so unpins) pinned threads.
- T3 stores no settle origin (manual vs auto).

### T3 visits (`apps/web/src/components/ChatView.tsx`, `apps/web/src/uiStateStore.ts`) **[verified: source]**
- `markThreadVisited` stamps `lastVisitedAt` while the thread's chat view is mounted.
- Completed-unseen = `latestTurn.completedAt > lastVisitedAt` (client-side, §3).

### T3 project monogram (`apps/web/src/projectIdentity.ts`) **[verified: source]**
- `words` = runs of letters/digits (after NFKC). `first` = the first glyph of the first word.
- `second` = the first digit after the first glyph of the first word; else, with 2+ words, the first glyph of the last word; else the last glyph of the first word.
- Upper-cased, first 2 glyphs. No words → `"PR"`.
- Colour: `seed = lowercase(trimmed name) || "project"`; `index = fold(0, |i, cp| (i * 31 + cp) % 18)` over code points, into gray red orange amber yellow lime green emerald teal cyan sky blue indigo violet purple fuchsia pink rose.
- The tile's text is `<colour>-400` on a 14% background of the same colour.
- Worked examples: `orb` → `OB`, rose (17); `paneru` → `PU`, fuchsia (15) — both match the user's T3 screenshot.

### T3 time labels **[verified: source]**
- Working duration (`Sidebar.logic.ts` `formatWorkingDurationLabel`): `<60 s → "Ns"`, `<60 min → "Nm"`, else `"Hh Mm"`.
- Relative (`timestampFormat.ts` `formatRelativeTime` + `Sidebar.tsx` `compactSidebarTimeLabel`): `<60 s` or negative → `"now"`, `<60 min → "Nm"`, `<24 h → "Nh"`, else `"Nd"`.

### T3 status pills (`Sidebar.logic.ts` `resolveThreadStatusPill`) **[verified: source]**
- Pending Approval: amber-300. Awaiting Input: indigo-300. Working: sky-300. Completed: emerald-300 (dark-mode classes).

### `claude` 2.1.282 **[verified]**
- `claude stop <id>`: ~0.73 s, prints `stopped <id>`. The record then has `state: "stopped"` and no `pid`/`status`; the conversation is kept. Help: "resume it later with `claude attach <id>`".
- `claude attach <id>` on a stopped session resumes it: a new pid, the same `id` and `sessionId`, `status: "idle"`, first byte after ~0.2 s.
- `claude rm <id>`: ~0.68 s, prints `removed <id>`. It works on a live session (kills it) and removes the record; the transcript under `~/.claude/projects/` stays. Help: "Delete a background session and its worktree. Unlike `stop`, works on already-exited sessions."

### Libraries
- **ratatui-which-key 0.14** (`src/state.rs`) **[verified: source]**:
  - `WhichKeyState` has `pub current_sequence: Vec<K>`.
  - `handle_key` pushes the key, then: a `Branch` stays pending (`active = true`); a `Leaf` returns its action and clears; no match dismisses the sequence and returns `None` (no catch-all handlers).
  - Backspace pops one pending key.

### Transcript `gitBranch` **[verified]**
- Present on `user` and `assistant` lines. An empty string means "no branch". The preview already takes the latest non-empty value (`Conversation::branch`).
