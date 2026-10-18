---
name: orb
description: Drive orb, the terminal multiplexer you run in. List sessions, panes and agents; split or stack a pane; run a command in a pane and read its screen; start another Claude or pi agent in a pane, prompt it, wait for it and read its reply; start a new session, or colonists from a colony. Use when the user mentions orb, asks to split a pane or run something in another pane, asks you to start, prompt, wait on or collect work from another agent, or asks you to spawn colonists, or to add, update or close an item on the user's plate (`orb plate`). Works only inside an orb pane (ORB_PANE_ID set).
---

# orb

- **You run in a pane of orb,** a terminal multiplexer for coding agents.
- **The `orb` command talks to the running orb.** One command sends one request and prints one JSON reply.

## First, check you're inside orb

- **Run `echo "$ORB_PANE_ID"`.** If it prints nothing, tell the user you're not running inside an orb pane, so you can't control orb, and stop.
- **Any command that fails with `not_running`** ("orb is not running") means no orb answers. Say so and stop.

## How commands answer

- **Success:** exit 0, the result as JSON on stdout. A change with nothing to report prints `{}`.
- **Refused:** exit 1, `{"code": "...", "message": "..."}` on stderr. Read the code (see "When orb says no").
- **Typed wrong:** exit 2 and the usage on stderr. Check your command against "Commands" below.
- **IDs are numbers,** plate item IDs too. A `[<pane>]` or `[<session>]` left out means your own pane or session.

## Rules

- **Never move the user's keys.** Every command leaves them alone except `orb pane focus` and `orb session focus`. Use those only when the user asks to be shown something.
- **Don't close, rename or settle what you didn't create,** unless the user asks.
- **Mind the defaults.** `orb tab close` with no `--pane` closes your own tab, and `orb session settle` with no ID settles your own session.
- **Don't close the last pane of the session the user is looking at.** orb opens a fresh shell there and moves the user's keys into it.
- **Settled sessions are read-only.** The lists, `agent get` and `agent reply` still work; everything else is refused. Don't try to un-settle one; that's the user's call.
- **There is no delete or quit.** orb can't delete a session or worktree, remove a project or quit through these commands. Don't look for a way.

## Look around

```sh
orb pane list               # {"panes": [...]}, your session
orb tab list                # {"session": 3, "tabs": [{"index": 1, "panes": [10, 11]}]}
orb session list            # {"sessions": [...]}, all of orb
orb agent list              # {"agents": [...]}, all of orb
orb agent get <pane>        # one agent row
```

- **`pane list` and `tab list` cover one session,** yours unless you add `--session <id>`. `session list` and `agent list` cover all of orb, settled sessions included.
- **A pane row has** `id, session, tab, float, name, cwd, agent, caller`. Your own pane has `"caller": true`. `agent` is the agent ID live in it, or null.
- **Tabs count from 1,** as the tab bar does. A floating pane has `"tab": null` and `"float": true`.
- **An agent row has** `id, pane, session, harness, name, title, status, cwd, branch`. `title` is the agent's own title once it has one. `status` is one of `unknown, working, needs_approval, needs_input, idle, failed, stopped, gone`.
- **A session row has** `id, project, name, dir, branch, settled`.

## Run a command in another pane

```sh
orb pane split --direction right        # {"pane": 12}
orb pane run 12 "just test"
orb pane read 12 --lines 200            # {"text": "..."}
```

- **`split` puts a new shell beside a pane** (yours by default), `right` or `down`. `orb pane stack [<pane>]` stacks one with it, `orb pane float` adds a floating one, `orb tab create` adds a tab. All reply `{"pane": N}` and leave the user's keys alone.
- **`run` types the line and Enter.** A pane that hasn't drawn yet gets it once it does, so no sleep is needed after `split`.
- **`read` is a snapshot.** `--lines 0` (the default) is the screen only; `--lines N` adds up to N lines of history above it. There's no "wait until output": poll `read` a few seconds apart.
- **`send` pastes text without Enter; `keys` presses named keys,** e.g. `orb pane keys 12 ctrl+c`. Key names: `enter esc tab backspace up down left right home end pageup pagedown`, `ctrl+<a-z>`, `alt+<a-z>`, `shift+tab`, any case. An unknown name sends nothing.
- **Text for `send` and `run` is one quoted argument,** taken as is even when it starts with `-`. Don't put `--` before it: only `agent start` and `agent prompt` accept `--`.

## Start another agent

```sh
orb pane split --direction right                          # {"pane": 12}
orb agent start --kind claude --pane 12 --name reviewer   # {"pane": 12, "agent": 40, "harness": "claude", "status": "idle"}
orb agent start --kind pi --pane 13 -- --model sonnet     # words after -- go to pi
```

- **It types `claude` or `pi` into a shell pane** and replies once orb sees the agent running (up to 30 s). Words after `--` are typed after the program, each quoted. `--name` names the pane. Always pass `--pane`: left out, it means your own pane, where you already run.
- **`pane_busy`:** an agent already runs there. Split a new pane.
- **`timeout`:** orb never saw it start. Either `orb integration install` hasn't run, or Claude is asking whether to trust a new folder. `orb pane read` the pane to see which, and tell the user.
- **The agent is named by its pane.** Use the pane ID for every `agent` command.

## Give it work and collect the answer

```sh
orb agent prompt 12 "Review the diff on this branch and list bugs." --wait   # {"status": "idle"}
orb agent reply 12                                                           # {"text": "...", "at": 1760000000000}
```

- **`--wait` returns when the turn ends** (`"idle"`) or the agent stops to ask the user (`"blocked"`). Add `--timeout <ms>` (only with `--wait`) to cap the whole call. No timeout waits as long as work goes on.
- **The reply comes from the transcript:** the last text the agent wrote in its last finished turn. It's not cut or wrapped like the screen. `at` is unix milliseconds, or null.
- **The prompt is one quoted argument.** If it starts with `-`, put the flags first and `--` before it: `orb agent prompt 12 --wait -- "--force is broken"`.
- **Prompting a busy agent queues the text.** `--wait` then returns at the end of the turn already running, so `orb agent wait 12` again before reading the reply.
- **Fan out:** start several agents, prompt each without `--wait` (replies `{"submitted": true}`), then `orb agent wait <pane> [--timeout <ms>]` and `orb agent reply <pane>` for each.
- **`orb agent get <pane>`** shows the agent's status and title any time.

## A whole new session

```sh
orb session create --worktree --name "fix login"   # {"session": 7, "pane": 30}
orb agent start --kind claude --pane 30             # its pane is a shell; start the agent there
orb agent prompt 30 "Fix the login bug."
```

- **It defaults to your session's project and its checkout.** `--worktree` makes a new worktree from the default branch (can take ~15 s). `--project <id>` picks another project.
- **Find a project ID in `orb session list`:** the `project` of a session whose `dir` is in that repo. There is no project list; if no session ever ran there, ask the user.
- **It shows in the sidebar but doesn't open,** so the user's view stays put.
- **One start at a time.** `starting` means another session is being made. Try again in a few seconds.

## Worlds, colonies and colonists

- **A world is a folder colonies run in.** In a shared world every colony works in the world's one folder. In a fresh world each colony gets its own copy of the world's folder. Research and Learn are orb's built-in fresh worlds (`~/.orb/research/<name>/`, `~/.orb/learn/<name>/`); the user's fresh worlds put theirs in `~/.orb/worlds/<world>/<name>/`.
- **A colony is a session in a world; its agent is the chief.** Only the user makes colonies, with `<C-g> c`. You can't.
- **A colonist is a session the chief, or a colonist, makes with `orb session create`.** orb links it under the session of the pane that ran the command (from `ORB_PANE_ID`), folded behind a `colonists` row in the sidebar, at any depth. From a normal session the new session is top-level.
- **"Spawn colonists" means sessions, never panes.** Run `orb session create` once per colonist from your own pane, then `orb agent start --kind claude --pane <pane>` and `orb agent prompt` on each returned pane. Splits, stacks and tabs stay in your session and never show as colonists; they're for helpers working beside you.

```sh
orb session create --name "scout 1"                              # fresh world: in your own folder
orb session create --project 4 --worktree --name "fix payout"    # a worktree of project 4's repo
```

- **Without `--project` it takes your session's project.** In a fresh world it runs in your own folder, with no worktree; in a shared world, in the world's folder; in a repo, in its checkout (add `--worktree` for its own).
- **For a repo, name it:** `--project <id> --worktree` gives the colonist its own worktree there.
- **Two refusals (`bad_request`):** `--worktree` without `--project` from a fresh world (its folders aren't git repositories), and `--project` naming a fresh world, from anywhere (its colonies come from `<C-g> c`).
- **Settling a colony doesn't settle its colonists.** `orb session settle` acts on one session only.

## The plate

```sh
orb plate add "Review PR #42: fix login" --verb review --link https://github.com/o/r/pull/42   # {"item": 7}
orb plate add "Try the new picker" --verb walk --what "Built on orb/picker; run just run." --link ./notes.md
orb plate add "Add a retry to payouts?" --verb idea
orb plate update 7 --waiting         # the next move is someone else's
orb plate done 7                     # {}
orb plate list                       # {"items": [...]}
```

- **The plate is the user's one list of what needs them,** across all of orb. orb shows it on the start screen and with `<C-g> o`.
- **Add one item when you end a turn that needs the user:** a decision, a review, something to read or try, a pick, an answer. Don't add items for work in progress, or for a permission or question prompt you're stuck on. orb adds its own row for an agent waiting for approval or an answer.
- **The title is the user's next step, as an action** ("Review PR #42: fix login"). `--what` says what happened in a sentence or two.
- **Pick the verb that fits.** Anything else is refused with the list.

| Verb | For |
| --- | --- |
| `decide` | Pick a direction. |
| `review` | A diff or PR. |
| `read` | A report or doc. |
| `walk` | Try it by hand. |
| `pick` | Choose among options. |
| `approve` | A plan or a step. |
| `answer` | A question you asked. |
| `reply` | Someone else's message. |
| `merge` | A branch that's ready. |
| `note` | An FYI. |
| `idea` | A follow-up you offer. |

- **Put the ticket, PR and page links on the item** with `--link`, which you can repeat. A web link needs its full address (`https://…`). Anything without `://` is a file, opened in `$EDITOR` (else `less`), except `.html` files, which open in the browser. A relative path starts at your session's folder.
- **One item per thing.** Update yours (`orb plate update <id>`) instead of adding a second. Find an ID with `orb plate list`. `--link` on update replaces all links; left out, they stay.
- **`--waiting`** when the next move is someone else's (CI, a reviewer). `--your-move` moves it back.
- **`--verb idea`** offers a follow-up. It lands in Parked until the user takes it up. An idea can't be `--waiting`.
- **`--urgent`** only when it blocks other work. Urgent items sort first.
- **Close your own item** with `orb plate done <id>` once the user's step is done. Nothing closes items by itself, not even settling or deleting the session.
- **Colonists don't add items.** orb refuses them with `colonist`. Put it in your report to your chief instead.
- **`--pane <id>`** sends the user's ⏎ to another pane, such as a tab running a prototype. By default it's your own pane.

## When orb says no

| Code | What it means | What to do |
| --- | --- | --- |
| `not_running` | No orb answers. | Tell the user and stop. |
| `no_target` | You left the target out and aren't in an orb pane. | Name the pane or session. |
| `not_found` | That pane, session, agent or plate item doesn't exist. | `orb pane list`, `orb agent list` or `orb plate list` and pick a real one. |
| `settled` | The session is settled. | Leave it alone; the user can un-settle it. |
| `colonist` | You're a colonist; colonists don't add plate items. | Put it in your report to your chief. |
| `starting` | Another session is being made. | Try again in a few seconds. |
| `pane_busy` | An agent already runs in that pane. | Split a new pane and start there. |
| `no_agent` | No live agent in that pane (or it stopped). | `orb agent get <pane>` to check; start one if you meant to. |
| `agent_blocked` | The agent waits for an approval or an answer. Nothing was typed. | Tell the user. Only if they asked you to handle it, `orb pane read` the dialog and answer with `orb pane keys`. |
| `prompt_stalled` | The prompt was typed but no work showed within 5 s. | `orb pane read` to see why before trying again. Don't paste the prompt twice. |
| `no_reply` | The agent hasn't finished a turn yet. | `orb agent wait <pane>`, then ask again. |
| `timeout` | Your `--timeout` ran out, `agent start` waited 30 s, or `session create` waited 3 min. | Check with `orb session list`, `orb agent get` or `orb pane read` before trying again; a session may already be there. |
| `bad_request` | The request makes no sense, e.g. splitting a floating pane or an unknown key name. | Read the message and fix the command. |
| `failed` | orb tried and something broke. | Read the message and tell the user. |

## Commands

```
orb session list [--all] [--project <id>]
orb tab list [--session <id>]
orb pane list [--session <id>]
orb agent list [--all] [--session <id>] [--status <status>[,<status>...]]
orb project list
orb session create [--project <id>] [--worktree] [--name <text>]
orb tab create [--session <id>] [--name <text>] [--run <command>] [--cwd <dir>]
orb pane split [<pane>] --direction right|down [--name <text>] [--run <command>] [--cwd <dir>]
orb pane stack [<pane>] [--name <text>] [--run <command>] [--cwd <dir>]
orb pane float [--session <id>] [--name <text>] [--run <command>] [--cwd <dir>]
orb pane close <pane>
orb pane rename <pane> <name>
orb pane focus <pane>
orb tab close [--pane <id>]
orb session rename [<session>] <name>
orb session settle [<session>]
orb session focus <session>
orb pane send <pane> <text>
orb pane keys <pane> <key>...
orb pane type <pane> <text>
orb pane run <pane> <command> [--wait] [--timeout <ms>] [--force] [--tail <n>] [--json]
orb pane read [<pane>] [--lines <n>] [--head <n>] [--tail <n>] [--json]
orb pane wait <pane> (--text <text> | --regex <pattern>) [--timeout <ms>] [--lines <n>] [--json]
orb agent start --kind claude|pi [--pane <id> | --split right|down | --tab] [--name <text>] [-- <args>...]
orb agent prompt <pane> <text> [--wait [--timeout <ms>]] [--json]
orb agent wait <pane> [--timeout <ms>]
orb agent get <pane>
orb agent reply <pane> [--json]
orb plate add <title> --verb <verb> [--what <text>] [--link <url>]... [--urgent] [--waiting] [--pane <id>]
orb plate update <item> [--title <text>] [--verb <verb>] [--what <text>] [--link <url>]... [--urgent|--not-urgent] [--waiting|--your-move]
orb plate done <item>
orb plate list
```

- **An empty name in `rename`** goes back to the usual name.
- **Splitting, stacking or closing the tab of a floating pane** is refused.
