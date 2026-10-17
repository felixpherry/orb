---
name: hand-off
description: Hands work to a fresh agent in its own orb session. Writes a handoff note, creates the session (with its own git worktree for code work, or as a colonist when called from a colony), starts Claude or pi with the chosen model and effort, and sends the note as the first prompt, including which pane to report back to. Use when the user says "hand this off" or wants work continued in a new session or worktree, or when another skill such as approve-plan needs to launch a runner.
argument-hint: "[what the new agent should do] [model] [effort]"
---

# orb handoff

Hand the current work to a new agent in its own orb session. Load the orb skill first. If `$ORB_PANE_ID` is empty, you're not in orb: say so and stop.

## 1. Write the handoff note

Write it to a file the new agent can read by absolute path: `.plans/<task>/handoff.md` when the work has a plan folder, else `/tmp/orb-handoff-<slug>.md`. Include:

- **Goal:** what the new agent must finish, and what done looks like.
- **State:** what's done, what's left, and the decisions made so far, with reasons.
- **Paths:** absolute paths to the plan, specs, and files that matter. A new worktree has no gitignored files (`.plans/`, `.env`), so point at the originals.
- **Skills:** which skills to load, and the exact first command if one applies (`/run-simple-loop /abs/path/.plans/<task>`).
- **Report back:** `orb agent prompt <your $ORB_PANE_ID> "<one line: result, plus the PR or file link>"`, run when done or stuck.

Reference specs, plans, commits and tickets by path or link instead of copying them. Leave out secrets: API keys, passwords, tokens.

## 2. Create the session

- **Code work in a repo:** `orb session create --worktree --name "<short name>"`. It branches a new worktree from the default branch (takes ~15 s).
- **Anything else:** `orb session create --name "<short name>"`.
- **Called from a colony:** the new session becomes a colonist on its own. There `--worktree` alone is refused (`bad_request`): find the repo's project id in `orb session list`, then run `orb session create --project <id> --worktree --name "<short name>"`.

The reply names the new session's shell pane: `{"session": 7, "pane": 30}`. The session doesn't open in the user's view. Never focus it.

## 3. Start the agent

Permission prompts stay off, so an unattended run never stalls.

```
orb pane rename <pane> "<short name>"
orb pane run <pane> "claude --dangerously-skip-permissions --model <model> --effort <level>"
```

On macOS, start the command with `caffeinate` (`"caffeinate claude ..."`) to keep the Mac awake while the agent works. It doesn't exist elsewhere.

Then run `orb agent get <pane>` every few seconds until it shows the agent (about 6 s). After 30 s without one, run `orb pane read <pane>`: Claude may be asking whether to trust the new folder. Tell the user; don't answer it for them.

- **Model and effort:** use what the user or the calling skill named. Otherwise leave both flags out, so the agent takes its defaults.
- **pi instead of Claude:** `orb agent start --kind pi --pane <pane> --name "<short name>" -- --model <model> --thinking <level>`.

## 4. Send the note

```
orb agent prompt <pane> "Read <absolute path to the note> and follow it."
```

Don't add `--wait`: the new agent works on its own.

## 5. Tell the user

Say in one line which session started, what its agent is doing, and that it reports back to this pane. When the report arrives, pass it on to the user in chat.
