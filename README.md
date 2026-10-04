# orb

A vim-first terminal manager for running many [Claude Code](https://claude.com/claude-code) and pi sessions at once, across projects and git worktrees.

## Quickstart

You need macOS or Linux, a recent stable Rust toolchain, [Claude Code](https://docs.claude.com/en/docs/claude-code), [zellij](https://zellij.dev) and a [Nerd Font](https://www.nerdfonts.com) in your terminal. [lazygit](https://github.com/jesseduffield/lazygit), [neovim](https://neovim.io) and, on macOS, [terminal-notifier](https://github.com/julienXX/terminal-notifier) are optional. For pi sessions you also need pi and zmx on your `PATH` when orb starts (`brew install neurosnap/tap/zmx` on macOS; on Linux, `paru -S zmx` from the AUR, or `paru -S zmx-bin` for the release build); without either, `␣h` shows pi disabled as `pi not found` or `zmx not found`.

On Linux, notifications need a freedesktop notification server, which most desktops and shells run; without one, orb shows none. On niri, clicking a notification also brings orb's window forward; on other compositors it only goes to orb's zellij tab and pane.

```sh
cargo install --git https://github.com/felixpherry/orb --locked
zellij   # orb opens shells, lazygit and nvim as zellij floating panes
orb
```

Then:

1. `␣p` adds a project from a directory picker (`Tab` opens a folder, `⏎` adds it).
2. `␣n` picks a project and opens a draft. `␣h`, `␣w`, `␣b`, `␣m` and `␣a` set its harness (Claude Code or pi), workspace, branch, model and permission.
3. `⏎` starts the session and attaches you to Claude or pi.
4. `<C-\>` takes you back to orb. The session keeps running, in Claude's background supervisor or, for pi, under zmx, even after you quit orb.
5. For related threads, `␣gf`, `␣gr` and `␣gl` make a Feature, Research or Learn group. For a quick question outside any project, `␣i` opens an Incognito draft.

## Demo

![orb demo](docs/demo/demo.gif)

[Watch it as an MP4](docs/demo/demo.mp4)

The demo shows, in order:

1. The sidebar and the dashboard start screen, then the which-key popup and its `+group` menu.
2. Approving a tool call and answering Claude's question, with both sessions attached at once.
3. Jumping back with `<C-o>`.
4. Search, rename and pin.
5. A Feature group: its card, folding it, and a new thread started with `n`.
6. Settling (with its `No`/`Yes` confirm), the Settled shelf, and deleting.
7. Hiding and resizing the sidebar.
8. Adding a project, and the `Initialize Git` offer for a folder that isn't a git repository.
9. Starting a session in a new worktree through the model, permission, workspace and branch pickers.
10. Lazygit, a shell and Neovim opened in that worktree.
11. Making a Feature group and a Research group, and starting the Research group's draft.
12. An Incognito session, with its `Trust /tmp/orb-incognito?` confirm.
13. The project filter (with Research and Incognito listed), removing a project, and filtering to one.

## Features

- **One list for every session.** The sidebar shows each thread's status (working, needs approval, needs input, done), its project, branch and elapsed time, across all projects. Pinned threads sit on top, and quiet ones settle onto a collapsible shelf.
- **The real Claude or pi, attached.** `⏎` runs `claude attach`, or `zmx attach` for a pi thread, in a terminal pane inside orb, so every slash command, permission prompt and picker works. Each attached thread keeps its own pane.
- **pi beside Claude.** `␣h` on a draft picks Claude Code or pi. A pi session runs under zmx, so it outlives orb too. orb reads its status (working, idle, stopped), its title (`/name`, else the first prompt), the session picker's preview and `␣sg`'s messages from pi's session file, and `␣m` lists the models `pi --list-models` printed at orb's start, under a heading per provider. pi has no permission modes, so `␣a` isn't offered on a pi draft, and orb's trust confirm doesn't apply to it. A lone pi thread shows a dim `pi` tag before its status.
- **Worktrees built in.** A new session can get its own worktree under `~/.orb/worktrees/`. Once Claude names the thread, its `orb/<hex>` branch is renamed to match the title. Every hour, and at start, orb prunes worktrees that nothing uses, or whose threads were all settled at least 7 days ago, keeping their branches and skipping any with uncommitted changes; `⏎` on a pruned thread recreates its worktree on its branch and resumes the chat.
- **Drafts.** Set up the project, harness, workspace, base branch, model and permission before starting a session. A draft in a folder that isn't a git repository starts in that folder, and `␣w`/`␣b` there offer to initialize git.
- **Trust handled for you.** When Claude refuses a folder it hasn't been trusted in, orb asks `Trust <path>?` (`No` selected). `Yes` marks it trusted in Claude's `.claude.json` and retries the start. pi threads never get this confirm.
- **Groups.** Threads that belong together share one directory and sit under one card: a **Feature** group's worktree (on a branch named after the group, like `GT-514-login`), or a **Research** or **Learn** folder under `~/.orb/research/` or `~/.orb/learn/`, copied from `~/.orb/templates/<kind>/` (orb writes a default there first: for Research, a research kit with an orchestrator `AGENTS.md`, investigator, falsifier and simulator subagents, conventions, a report template and `SOURCES.md`; for Learn, a learning kit with an orchestrator `AGENTS.md`, a saboteur subagent, the storyteller, challenge, tutor and conventions files and an epub ingest script). The Research and Learn kits' subagents work under Claude Code and pi. Under pi they need pi's example `subagent` extension, installed once at user level (orb doesn't install it; the commands are in the kit's `AGENTS.md` and `docs/research.md` §20), and if you use pi-amplike that install also changes its `"npm:pi-amplike"` entry in `~/.pi/agent/settings.json` to `{"source":"npm:pi-amplike","extensions":["!extensions/subagent.ts"]}`, because amplike has its own `subagent` tool and pi won't start with both. Each group has a default harness, model and permission, set with `␣h`/`␣m`/`␣a` on its card; threads already running keep theirs. `n` in an active group opens its draft, making one at the top of the group when it has none. The draft follows the card's defaults except for each one you pick on it with `␣h`/`␣m`/`␣a`, and `⏎` starts it in the group's folder or worktree. Once the group has a thread, `d` discards its draft. `␣b` on a started Feature group's card switches its worktree's branch, which moves every thread in it. A name another group has, or whose branch or folder already exists, is refused with the name box still open. Groups are pinned, settled and deleted as a whole. Deleting one also removes its folder, or its worktree (even with changes) and the branch orb made for it, which frees the name. A Feature group whose branch has unmerged commits (as `git branch -d` sees it) isn't deleted at all, and the mode line says `branch <name> has unmerged commits`.
- **Incognito.** `␣i` (or `i` on the dashboard) opens the Incognito draft, for a Claude session outside any project, with no picker step; `⏎` starts it, as on any draft. Every incognito thread runs in orb's `Incognito` project at `/tmp/orb-incognito/`, which orb creates at start and again before each incognito start. The first one asks `Trust /tmp/orb-incognito?` once (orb writes Claude's trust for its realpath, `/private/tmp/orb-incognito` on macOS), and later ones don't. Incognito threads are normal threads: they settle, delete, rename and resume like any other. `␣n` doesn't list Incognito, and `␣f` lists it after Research and Learn. `␣w`/`␣b` aren't bound on its rows, and its draft never shows Workspace or Branch, even if the folder becomes a git repository.
- **Find and jump.** `/` searches thread titles and group names, `␣f` narrows the sidebar to one project, and `<C-o>`/`<C-i>` move back and forward through a jump list, as in neovim. `␣␣` (or `<C-Space>` in the Claude pane) opens a session picker over your other threads, newest chat first, with a preview of each chat; `⏎` jumps into the thread's pane. `␣sg` searches what was said in every thread, your prompts and Claude's or pi's replies, newest message first, and previews the exchange around each match.
- **Tools where the code is.** `␣t` opens a shell, `␣gg` lazygit and `␣v` `nvim .` in the thread's directory, as a full-screen zellij floating pane.
- **Notifications.** When orb isn't focused, you get a desktop notification when a thread finishes, needs approval or needs input (a pi thread only ever finishes). On macOS it's sent through terminal-notifier, whose click takes you back to orb's zellij tab and pane, or through `osascript` when terminal-notifier isn't installed. On Linux it goes to the desktop's notification server: approval and input are critical, a newer notice about a thread replaces the last one, and a click takes you back to orb's tab and pane (on niri, bringing orb's window forward too).
- **LazyVim look.** A snacks-style explorer, a dashboard start screen, a lualine mode line, a helix which-key popup, vim.ui.select pickers and snacks session, worktree and search pickers, all in tokyonight-moon.

## Keys

`<Space>` is the leader. Press it and wait to see the which-key popup.

| Key | Action |
| --- | --- |
| `j` / `k`, `gg` / `G`, `<C-d>` / `<C-u>` | Move in the sidebar |
| `⏎` | Attach to the thread, start the draft, or open/close the group |
| `<C-\>` | Detach and go back to orb |
| `<C-b>` | In the Claude pane, hide or show the sidebar; the keys stay in the pane (Claude's background-task key is then `Ctrl+X Ctrl+B`) |
| `<C-h>` / `<C-l>` | Focus the sidebar / the right-hand side |
| `<C-o>` / `<C-i>` | Jump back / forward through the jump list (sidebar, dashboard and the Claude pane) |
| `␣␣` / `<C-Space>` | Session picker: threads newest chat first, with a preview of the selected chat (`<C-Space>` in the Claude pane; `<C-s>` shows or hides settled threads) |
| `␣sw` | Worktree picker: every worktree under `~/.orb/worktrees/`, with its size, changes, users and when the sweep will prune it |
| `<C-x>` | In the worktree picker, delete the worktree, changes included, after a `No`/`Yes` confirm; the branch stays, and a worktree whose thread is attached or mid-turn can't be deleted |
| `␣sg` | Search picker: every prompt and reply in orb's threads, newest first, with the exchange around the match (`⏎` jumps into the thread's pane) |
| `/` or `i` | Search thread titles and group names |
| `r` | Rename the thread |
| `l` / `h` | Open / close the group (or the Settled shelf) |
| `n` | Open the group's draft, making one at the top of the group when it has none; not on a settled group |
| `p` | Pin or unpin (a lone thread or a group) |
| `s` | Settle (after a `No`/`Yes` confirm) or un-settle (a lone thread or a group) |
| `d` | Delete the thread or the group, or discard the draft (after a `No`/`Yes` confirm); a group's last thread can't be deleted, nor the draft of a group with no thread |
| `␣n` | New session (project picker) |
| `␣i` | Open the Incognito draft in `/tmp/orb-incognito/` (`⏎` starts it) |
| `␣gf` / `␣gr` / `␣gl` | New Feature (project picker, then a name) / Research / Learn group |
| `␣p` | Add a project |
| `␣f` | Filter by project, with Research, Learn and Incognito right after All projects (`<C-x>` removes one) |
| `␣w` / `␣b` | Workspace / branch picker (not in a group or the Incognito project; `␣b` also on a started Feature group's card) |
| `␣h` | Harness picker: Claude Code or pi (drafts; on a group's card, the group's default; on a group's draft, its own) |
| `␣m` / `␣a` | Model / permission picker (drafts; on a group's card, the group's defaults; on a group's draft, its own; `␣a` not on pi) |
| `␣t` / `␣gg` / `␣v` | Shell / lazygit / Neovim |
| `␣e` | Hide or show the sidebar |
| `<C-Left>` / `<C-Right>` | Narrow / widen the focused side |
| `q` | Quit orb (sessions keep running) |

On the dashboard, each menu item's letter runs it directly (there `i` is Incognito; in the sidebar it searches), and `j`/`k` plus `⏎` work too.

`<C-o>`/`<C-i>` work like neovim's jump list. Entering a thread's pane, `gg`/`G`, a search `⏎` and a `␣n` pick are jumps; `j`/`k` and `<C-d>`/`<C-u>` aren't. Entering the pane of the row the last `<C-o>`/`<C-i>` landed on isn't a new jump, so `<C-i>` still goes forward. orb keeps the newest 20 rows, each at most once, across restarts. A jump moves the cursor, and shows the thread's pane only while orb is still attached to it. It never attaches, starts a draft or clears the project filter.

Inside the Claude pane orb takes `<C-o>`, which is Claude's transcript key. To keep the transcript on `ctrl+shift+o`, add it to `~/.claude/keybindings.json` (`ctrl+o` stays bound for `claude` outside orb):

```json
{
  "$schema": "https://www.schemastore.org/claude-code-keybindings.json",
  "$docs": "https://code.claude.com/docs/en/keybindings",
  "bindings": [
    { "context": "Global", "bindings": { "ctrl+shift+o": "app:toggleTranscript" } }
  ]
}
```

kitty maps `ctrl+shift+o` to `pass_selection_to_program` by default and swallows it, so also add `map ctrl+shift+o no_op` to `kitty.conf` and reload kitty (`ctrl+shift+F5`).

## Mouse

orb captures the mouse from the moment it starts.

- A click selects a sidebar row or a picker row. A double-click, two clicks on the same row within 500 ms, acts as `⏎`: it attaches, starts the draft, opens or closes the group or the Settled shelf, or picks the picker row.
- A click on a sidebar row moves the keys to the sidebar, and a click on the right-hand side moves them to the dashboard or into the Claude pane. The click that moves them into the pane isn't sent to Claude; later clicks and the wheel over the pane are.
- With the keys in the sidebar, the wheel moves the selection one row. Otherwise, with no picker or rename box open, the wheel over the sidebar scrolls only its view, 3 lines a notch. Over a picker's list it moves the picker's selection, and over the session, worktree or search picker's preview it does nothing. Neither wraps.
- A click on a dashboard item highlights it and doesn't run it.
- A click outside a picker or the rename box closes it, as `Esc` does.
- A click on the sidebar's search box starts a search, and a click in any input's text moves the cursor there.

To select text with the mouse, hold Shift while you drag. That's kitty's default; other terminals have their own key.

## Development

```sh
just test   # cargo test --workspace
just lint   # check, clippy -D warnings, fmt check, test-attribute guard
```

orb stores its state in `~/.orb/userdata/state.sqlite` and its transcript search index in `~/.orb/userdata/search.sqlite`, which it rebuilds when the file is missing.
