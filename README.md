# orb

A vim-first terminal manager for running many [Claude Code](https://claude.com/claude-code) sessions at once, across projects and git worktrees.

## Quickstart

You need macOS, a recent stable Rust toolchain, [Claude Code](https://docs.claude.com/en/docs/claude-code), [zellij](https://zellij.dev) and a [Nerd Font](https://www.nerdfonts.com) in your terminal. [lazygit](https://github.com/jesseduffield/lazygit), [neovim](https://neovim.io) and [terminal-notifier](https://github.com/julienXX/terminal-notifier) are optional.

```sh
cargo install --git https://github.com/felixpherry/orb --locked
zellij   # orb opens shells, lazygit and nvim as zellij floating panes
orb
```

Then:

1. `␣p` adds a project from a directory picker (`Tab` opens a folder, `⏎` adds it).
2. `␣n` picks a project and opens a draft. `␣w`, `␣b`, `␣m` and `␣a` set its workspace, branch, model and permission.
3. `⏎` starts the session and attaches you to Claude.
4. `<C-\>` takes you back to orb. The session keeps running in Claude's background supervisor, even after you quit orb.

## Demo

![orb demo](docs/demo/demo.gif)

[Watch it as an MP4](docs/demo/demo.mp4)

The demo shows, in order: the sidebar and dashboard, the which-key popup, approving a tool call, answering Claude's question, two attached sessions at once, search, rename, pin, settling and the Settled shelf, deleting, hiding and resizing the sidebar, the project filter, adding a project, starting a session in a new worktree (model, permission, workspace and branch pickers), and then Neovim, lazygit and a shell opened in that worktree.

## Features

- **One list for every session.** The sidebar shows each thread's status (working, needs approval, needs input, done), its project, branch and elapsed time, across all projects. Pinned threads sit on top, and quiet ones settle onto a collapsible shelf.
- **The real Claude, attached.** `⏎` runs `claude attach` in a terminal pane inside orb, so every slash command, permission prompt and picker works. Each attached thread keeps its own pane.
- **Worktrees built in.** A new session can get its own worktree under `~/.orb/worktrees/`. Once Claude names the thread, its `orb/<hex>` branch is renamed to match the title.
- **Drafts.** Set up the project, workspace, base branch, model and permission before starting a session.
- **Groups.** Threads that belong together share one directory and sit under one card: a **Feature** group's worktree (on a branch named after the group, like `GT-514-login`), or a **Research** or **Learn** folder under `~/.orb/research/` or `~/.orb/learn/`, copied from `~/.orb/templates/<kind>/` (orb writes a default there first: for Research, a research kit with an orchestrator `AGENTS.md`, investigator, falsifier and simulator subagents, conventions, a report template and `SOURCES.md`; for Learn, an `AGENTS.md`). Each group has a default model and permission, set with `␣m`/`␣a` on its card or draft. `n` starts another thread in an active group at once with those defaults (on a group that has only its draft, `⏎` on the draft starts it first); threads already running keep theirs. `␣b` on a started Feature group's card switches its worktree's branch, which moves every thread in it. A name another group has, or whose branch or folder already exists, is refused with the name box still open. Groups are pinned, settled and deleted as a whole. Deleting one also removes its folder, or its worktree (even with changes) and the branch orb made for it, which frees the name. A Feature group whose branch has unmerged commits (as `git branch -d` sees it) isn't deleted at all, and the mode line says `branch <name> has unmerged commits`.
- **Tools where the code is.** `␣t` opens a shell, `␣gg` lazygit and `␣v` `nvim .` in the thread's directory, as a full-screen zellij floating pane.
- **Notifications.** When orb isn't focused, you get a macOS notification when a thread finishes, needs approval or needs input.
- **LazyVim look.** A snacks-style explorer, a dashboard start screen, a lualine mode line, a helix which-key popup and vim.ui.select pickers, all in tokyonight-moon.

## Keys

`<Space>` is the leader. Press it and wait to see the which-key popup.

| Key | Action |
| --- | --- |
| `j` / `k`, `gg` / `G`, `<C-d>` / `<C-u>` | Move in the sidebar |
| `⏎` | Attach to the thread, start the draft, or open/close the group |
| `<C-\>` | Detach and go back to orb |
| `<C-h>` / `<C-l>` | Focus the sidebar / the right-hand side |
| `<C-o>` / `<C-i>` | Jump back / forward through the jump list (sidebar, dashboard and the Claude pane) |
| `/` or `i` | Search thread titles and group names |
| `r` | Rename the thread |
| `l` / `h` | Open / close the group (or the Settled shelf) |
| `n` | New thread in the group, with the group's default model and permission; not on a settled group, and on a group with only its draft it points at the draft |
| `p` | Pin or unpin (a lone thread or a group) |
| `s` | Settle (after a `No`/`Yes` confirm) or un-settle (a lone thread or a group) |
| `d` | Delete the thread or the group, or discard the draft (after a `No`/`Yes` confirm); a group's last thread can't be deleted |
| `␣n` | New session (project picker) |
| `␣gf` / `␣gr` / `␣gl` | New Feature (project picker, then a name) / Research / Learn group |
| `␣p` | Add a project |
| `␣f` | Filter by project, with Research and Learn right after All projects (`<C-x>` removes one) |
| `␣w` / `␣b` | Workspace / branch picker (not in a group; `␣b` also on a started Feature group's card) |
| `␣m` / `␣a` | Model / permission picker (drafts; on a group's card or draft, the group's defaults) |
| `␣t` / `␣gg` / `␣v` | Shell / lazygit / Neovim |
| `␣e` | Hide or show the sidebar |
| `<C-Left>` / `<C-Right>` | Narrow / widen the focused side |
| `q` | Quit orb (sessions keep running) |

On the dashboard, each menu item's letter runs it directly, and `j`/`k` plus `⏎` work too.

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

## Development

```sh
just test   # cargo test --workspace
just lint   # check, clippy -D warnings, fmt check, test-attribute guard
```

orb stores its state in `~/.orb/userdata/state.sqlite`.
