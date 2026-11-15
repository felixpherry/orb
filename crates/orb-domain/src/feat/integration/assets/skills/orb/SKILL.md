---
name: orb
description: Drives orb, the terminal multiplexer you run in, covering its panes, tabs, sessions, other agents, colonists and the user's plate. Use when the user mentions orb, asks to split a pane or run something in another pane, asks you to start, prompt, wait on or collect work from another agent, to spawn colonists, or to add, update or close an item on the user's plate (`orb plate`).
---

# orb

You run in a pane of orb, a terminal multiplexer for coding agents. The `orb` command sends one request to the running orb and prints one reply.

- **The help is the command reference.** `orb help` lists the nouns, `orb <noun> --help` a noun's verbs, and `orb <noun> <verb> --help` the flags, defaults, examples and the reply. Read a verb's help before its first use; this skill holds the rules and what the help can't tell you.
- **No orb, no work.** If `$ORB_PANE_ID` is empty, or a command fails with `not_running`, tell the user you can't control orb and stop. Bare `orb` prints your pane, tab and session.

## How commands answer

- **Exit 0: done.** The reply is JSON on stdout, `{}` when there is nothing to report. Commands that read a screen or an agent's answer print plain text; `--json` works on every command.
- **Exit 1: refused.** `{"code", "message", "hint"}` on stderr. The hint names the command that fixes it.
- **Exit 2: typed wrong.** It prints the usage line and the help command to run.
- **IDs are numbers.** A `[<pane>]` or `[<session>]` left out means your own.
- **Lists leave out settled sessions, dead agents and done plate items.** `hidden` counts them and `--all` shows them.

## Rules

- **Never move the user's keys.** Only `orb pane focus` and `orb session focus` do, so use them only when the user asks to be shown something.
- **Don't close, rename or settle what you didn't create,** unless the user asks.
- **Mind the defaults.** `orb tab close` with no `--pane` closes your own tab, and `orb session settle` with no ID settles your own session.
- **Don't close the last pane of the session the user is looking at.** orb opens a fresh shell there and moves the user's keys into it.
- **Settled sessions are read-only.** The lists, `agent get` and `agent reply` still work. Un-settling is the user's call.
- **orb can't delete a session or worktree, remove a project, or quit.** Don't look for a way.
- **Wait, don't poll.** Use `--wait`, `orb pane wait` or `orb agent wait`, never a loop of sleep and read.
- **Don't `--force` into a program you didn't start.**

## Run a command and get its result

```sh
orb pane split --direction right --name tests   # {"pane": 12}
orb pane run 12 "just test" --wait              # its output, then [exit 0]
```

- **`--name`, `--run` and `--cwd` work on `tab create`, `pane split`, `pane stack` and `pane float`.** A new pane takes `run` before it has drawn, so don't sleep after a split.
- **Exit codes need fish.** Other shells send no prompt marks, so `--wait` returns once the screen is quiet and prints `[exit ?]`, and `busy` is never checked.
- **`not_run`: the shell took the line but didn't run it.** It rejected the line or waits for the rest of it (an open quote or bracket). If the line still sits at the prompt, clear it with `orb pane keys <pane> ctrl+c`. Then fix the command and run it again.
- **A `timeout` saying it may still run** means orb couldn't tell. `orb pane read` the pane, press ctrl+c only if the line still sits at the prompt, and don't run the line again.

## Servers, REPLs and editors

A program that never ends makes `--wait` run to its timeout. Start it with plain `run` or `--run`, then wait for its text or type into it.

```sh
orb tab create --name server --run "just run"            # {"pane": 14}
orb pane wait 14 --text "listening on" --timeout 60000   # the first matching line
orb pane read 14 --tail 40

orb pane run 12 "python3"
orb pane type 12 "print(6*7)" --enter --read             # the screen once it settles, showing 42
orb pane keys 12 ctrl+d
```

- **`pane wait` also matches text already on screen and the line you typed,** so wait for text only the output prints. For a program that echoes what you type, use `type --enter --read`.
- **`type` presses each character, `send` pastes, and `keys` presses named keys** (`esc`, `ctrl+c`, `f5`). In vim, `keys` changes mode and `type` enters text.
- **`busy` means a program runs in that pane.** Type a line into it, quit it with `keys`, or ask the user.

## Other agents

```sh
orb agent start --kind claude --split right --name reviewer   # {"pane": 15, ...}
orb agent prompt 15 "Review the diff on this branch and list bugs." --wait   # the answer
orb agent start --kind pi --tab -- --model sonnet             # words after -- go to pi
```

- **Give `start` a place:** `--split right|down`, `--tab`, or `--pane <id>` for a shell pane you have. With none it means your own pane, which is refused as `busy`.
- **The agent is named by its pane.** Every `agent` command takes the pane ID.
- **To fan out, prompt each agent without `--wait`,** then `orb agent wait <pane>` and `orb agent reply <pane>` on each in turn.
- **Prompt an agent that is idle.** A prompt sent mid-turn may be folded into the running turn, so `--wait` ends early. If the answer doesn't cover your prompt, `orb agent wait` and read the reply again.
- **Long answers keep their last 12,000 characters.** `--full` prints it all.
- **`timeout` on start:** orb never saw the agent. Either `orb integration install` hasn't run, or Claude is asking whether to trust a new folder. `orb pane read` the pane and tell the user which.
- **`agent_blocked`:** it stopped to ask the user. Tell them. Only if they asked, read the dialog and answer with `orb pane keys`.
- **`turn_failed`:** its model call failed (quota, API error). Tell the user.
- **`prompt_stalled`:** `orb pane read` to see why. Don't paste the prompt twice.

## Find things

```sh
orb agent list --status needs_approval,needs_input
orb session list --all
orb project list
orb pane list --session 7     # yours when left out; `tab list` too
orb agent get 15              # adds the full name, cwd and branch
```

## A browser in a pane

```sh
orb pane browser 42                                        # {"key": "90107-1"}
terminal-browser action --browser 90107-1 -- snapshot
terminal-browser action --browser 90107-1 done
```

- **Find the pane with `orb tab list`,** which covers every tab of the session, not only the one the user sees.
- **Pass the returned `--browser` key on every action, `done` included.** Without it terminal-browser picks among all the session's browsers.
- **Run browser actions from your own command tool.** `orb pane run` into a browser pane types shell text into the page.

## A new session

```sh
orb session create --worktree --name "fix login"   # {"session": 7, "pane": 30}
orb agent start --kind claude --pane 30            # its pane is a shell
orb agent prompt 30 "Fix the login bug."
```

- **It takes your session's project and checkout.** `--worktree` makes a worktree from the default branch (about 15 s), and `--project <id>` picks another project from `orb project list`.
- **It shows in the sidebar without opening.**
- **`starting` means another session is being made.** Try again in a few seconds. On a `timeout`, check with `orb session list` first: the session may already exist.

## Worlds, colonies and colonists

- **A world is a folder colonies run in.** A shared world gives every colony its one folder. A fresh world gives each colony its own copy: `~/.orb/research/<name>/` and `~/.orb/learn/<name>/` for the built-in Research and Learn worlds, `~/.orb/worlds/<world>/<name>/` for the user's.
- **A colony is a session in a world, and its agent is the chief.** Only the user makes colonies.
- **A colonist is a session made with `orb session create` from a colony or from another colonist.** orb links it under the session that ran the command. From a normal session the new session is top-level.
- **"Spawn colonists" means sessions, never panes.** Run `orb session create` once per colonist, then `agent start --pane` and `agent prompt` on each returned pane. Splits and tabs are for helpers working beside you.
- **Without `--project` a colonist works in your folder** (a fresh world) or the world's folder (a shared one), with no worktree. For code work in a repo, pass `--project <id> --worktree`.
- **Refused:** `--worktree` without `--project` from a fresh world, and `--project` naming a fresh world.
- **Settling a colony doesn't settle its colonists.**

## The plate

```sh
orb plate add "Review PR #42: fix login" --verb review --what "Tests pass." --link https://github.com/o/r/pull/42   # {"item": 7}
orb plate add "Add a retry to payouts?" --verb idea
orb plate update 7 --waiting
orb plate done 7
```

- **The plate is the user's one list of what needs them,** across all of orb.
- **Add one item when you end a turn that needs the user:** a decision, a review, something to read or try, an answer. Not for work in progress, and not for a permission or question prompt, which orb lists itself.
- **The title is the user's next step, as an action.** `orb plate add --help` lists the verbs; `idea` offers a follow-up and lands parked.
- **One item per thing.** Update yours instead of adding a second, and close it with `orb plate done <id>` once the user's step is done. Nothing closes items by itself.
- **Put the ticket, PR and page on it with `--link`.** A web link needs `https://`; anything else is a file. On update, `--link` replaces every link.
- **`--waiting` when the next move is someone else's** (CI, a reviewer), `--urgent` only when it blocks other work.
- **`--pane <id>` sends the user's ⏎ to another pane,** such as a tab running a prototype.
- **Colonists can't add items** (`colonist`). Report to your chief instead.
