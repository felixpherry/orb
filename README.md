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
- **Groups.** Threads that belong together share one directory and sit under one card: a **Feature** group's worktree (on a branch named after the group, like `GT-514-login`), or a **Research** or **Learn** folder under `~/.orb/research/` or `~/.orb/learn/`, copied from `~/.orb/templates/<kind>/` (orb writes a default `AGENTS.md` there first). `n` starts another thread in the group at once. Groups are pinned, settled and deleted as a whole; deleting one leaves its folder and branch on disk, so the same name is refused until you remove them.
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
| `/` or `i` | Search thread titles and group names |
| `r` | Rename the thread |
| `l` / `h` | Open / close the group (or the Settled shelf) |
| `n` | New thread in the group, with the selected (on the card, the newest) thread's model and permission |
| `p` | Pin or unpin (a lone thread or a group) |
| `s` | Settle (after a `No`/`Yes` confirm) or un-settle (a lone thread or a group) |
| `d` | Delete the thread or the group, or discard the draft (after a `No`/`Yes` confirm); a group's last thread can't be deleted |
| `␣n` | New session (project picker) |
| `␣gf` / `␣gr` / `␣gl` | New Feature (project picker, then a name) / Research / Learn group |
| `␣p` | Add a project |
| `␣f` | Filter by project (`<C-x>` removes one) |
| `␣w` / `␣b` | Workspace / branch picker (not in a group) |
| `␣m` / `␣a` | Model / permission picker (drafts, a group's too) |
| `␣t` / `␣gg` / `␣v` | Shell / lazygit / Neovim |
| `␣e` | Hide or show the sidebar |
| `<C-Left>` / `<C-Right>` | Narrow / widen the focused side |
| `q` | Quit orb (sessions keep running) |

On the dashboard, each menu item's letter runs it directly, and `j`/`k` plus `⏎` work too.

## Development

```sh
just test   # cargo test --workspace
just lint   # check, clippy -D warnings, fmt check, test-attribute guard
```

orb stores its state in `~/.orb/userdata/state.sqlite`.
