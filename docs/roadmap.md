# orb — Overhaul roadmap

> The architecture overhaul after orb became an agent multiplexer (2026-10). This file is the overhaul session's working state: fill each section as the session reaches it.
> Product spec: [`product.md`](product.md). Decisions already made: [`decisions.md`](decisions.md). Current state: `.agents/RECORD.md`. The pre-overhaul roadmap (milestones 0–10) is in git history.

## Status

Phases: 1 debt audit → 2 falsify findings → **checkpoint 1** (user ranks findings) → 4 reference scouts (jinn, herdr, zellij) → 5 draft target → 6 falsify target → **checkpoint 2** (user accepts decisions one by one) → 8 migrate one step at a time (observability, then state and concurrency, then communication and workers, then crate and file boundaries), **checkpoint 3** after each step.

- **Done:** Phase 1 (2026-10-11): 13 audits, ~130 raw findings, grouped into 27 clusters. Phase 2: the falsifier kept 17 (below), sent 9 bugs to the backlog, dropped 13.
- **Checkpoint 1 (2026-10-11):** the user kept all 17 findings in the order below (noticeable ones F-1…F-16 in that order; the six invisible ones F-6, F-12, F-13, F-14, F-15, F-17 ordered by the orchestrator) and asked to fix F-1's triggers and F-3's branch bug now, ahead of the migration order.
- **Done:** Phase 4: 8 scouts wrote ~90 pattern cards to `scouts/` (jinn ×2, herdr ×2, zellij ×4).
- **Pre-migration fix:** commit 1a6728a (F-1 triggers + F-3 branch bug; "couldn't read or check never means gone"). The branch was renamed by orb to `orb/simplifying-technical-explanation`.
- **Done:** Phase 5 `target.md` (16 decisions) and Phase 6 `target-falsified.md` (verdicts: D-1 merged into D-14; D-3, D-4, D-6, D-11, D-12, D-13, D-15 shrunk; D-7's worker-report half is a spike: A report channel vs B local fixes, built in two throwaway worktrees).
- **Checkpoint 2 (2026-10-11):** the user accepted all 17 decisions below, with answers to each question. Spike A (report channel) chosen over B.
- **Next:** the Phase 2 Record entries in `.plans/overhaul-1-safety-net/record-proposal.md` await the user's OK; then migration step 3 (`/phased-task-loop .plans/overhaul-1-safety-net phase 3 only`). Each step runs as its own small plan: tests first, change, `just test` + `just lint`, RECORD facts, Drift log entry if the step left the target, commit, then **checkpoint 3**: stop and tell the user in plain words what changed and what to try.
- **Plans (2026-10-11):** the 18 steps are split into five phased plans in `.plans/` of worktree `orb-696e18ac`, run in order, one phase (= one step) per `/phased-task-loop … phase N only` so checkpoint 3 follows each: `overhaul-1-safety-net` (steps 1–5), `overhaul-2-less-waiting` (6–9), `overhaul-3-pane-records` (10–12, **effort high**), `overhaul-4-messaging` (13–15, **effort high**), `overhaul-5-code-shape` (16–18).
- **Step 1 done (2026-10-11, 728ede1):** AGENTS.md §3 describes today's flow, a name check in `just lint` keeps AGENTS.md and the Record honest, and the user-approved Record corrections are applied (2be25d0).
- **Step 2 done (2026-10-11):** orb writes `~/.orb/orb.log` (rolled to `orb.log.1` at 5 MB): a line per start, every kill, worktree prune, branch delete, session delete and dropped pane row, and the cause chain of every shown or dropped save, git, zmx, pane start and search error; the start screen names the session that exited at start (B-7); `just lint` rejects new `let _ =` kills/removals and `|_report|` drops.
- **Reference worktrees (throwaway, keep until step 15 is done):** spike A at `~/dev/orb/.claude/worktrees/agent-aa9053172de9c6e06` (branch `worktree-agent-aa9053172de9c6e06`, commit 8a56291, on 46c82e6); spike B at `~/dev/orb/.claude/worktrees/agent-ace3f1d0644bc8968`.
- **Working files:** `.plans/architecture-overhaul/` in worktree `orb-696e18ac` (git-ignored): `brief.md` (shared agent brief), `audit/*.md` (one file per audit agent), `clusters.md` (duplicate map), `falsified.md` (surviving findings). Only accepted results are copied here.

## Goal

Fit orb's architecture to what orb is today, a multiplexer, and keep it easy to change. Covers code shape (file and crate boundaries), concurrency (locks, threads, actors) and observability (seeing what is running or stuck).

## Debt findings

Survivors of the falsifier, with verified evidence in `.plans/architecture-overhaul/falsified.md`. Kind · fix size.

1. **F-1 orb treats "not in memory right now" as "gone for good".** A database read error at start, one unreadable saved tab, or a failed zmx check while a pane client exits makes orb kill live pane programs, delete pane rows and prune clean worktrees; bit twice (9dc4bc5, e0f1b01). Data safety · S to plug, M to fix. (sessions_actor.rs:2809-2846, run.rs:1124-1127 and 1402-1422, worktrees_actor.rs:153-155, store.rs:788-804)
2. **F-2 Database upgrades run on the only copy.** Only pre-v11 databases get a backup; v12 and v13 ran without; branch builds migrate the live database. Data safety · S. (store.rs:1099-1146, main.rs:59-61)
3. **F-3 The model is still "projects own threads".** Pane facts in four hand-synced copies; the branch rename updates only the thread, so the tab bar shows a dead branch and a restored worktree lands on a fresh branch. Wrong behaviour + friction · S for the branch bug, L for the model. (sessions_actor.rs:1645-1652 and 2603-2616, tabs.rs:313-320, sessions/state.rs:436-447)
4. **F-4 One sessions worker queues almost everything.** 27 message kinds one at a time; splits wait behind polls, git fetch and worktree add; most git calls have no time limit. Freezes · M. (run.rs:885-923, sessions_actor.rs:1572-1597 and 2107-2147, git_cli.rs:63-72)
5. **F-5 A panic off the main thread leaves orb half-alive.** Terminal restored while orb runs on, or a dead worker goes silent; poisoned locks ignored. Wrong behaviour, rare · S. (outer_terminal.rs:66-72, common/state.rs:25-32, sessions_actor.rs:372)
6. **F-6 A pane's life has two owners** (screen loop and sessions worker), coordinated by write order. Risk + friction · L. (run.rs:276-295, 1113-1235; sessions_actor.rs:2025-2072)
7. **F-7 Workers reach the screen through shared notes and replayed intents.** Attach slot, picker with four writers, a borrowed flag as a lock. Small wrong behaviour + friction · M. (run.rs:1300-1342, intent_handler.rs:63-66, worktrees_actor.rs:250-257, search_actor.rs:233-237)
8. **F-8 The screen thread waits on git and zmx.** Branch picker ~80 ms per open, zmx calls sleep in 10 ms steps, no time limits. Freezes · S to cap, M to move. (git_cli.rs:124-141, child.rs:36-46, run.rs:1194-1198)
9. **F-9 Every pane of every open session is live all the time.** Blank start-up until all reconnect; any pane's output redraws everything; no cap. Speed · M. (run.rs:561-563 and 838, pane.rs:315-326)
10. **F-10 No record when things go wrong.** No log, causes dropped, one fixed save-error text for 20 failures, removals unrecorded. Observability · S. (Cargo.toml:113-114, sessions_actor.rs:126, run.rs:1492-1495)
11. **F-11 Quit has no shutdown step.** Queued saves dropped, quit can hang on blocking work, no signal handling. Small data loss + friction · S. (run.rs:598-600, main.rs:59-63)
12. **F-12 Every feature lands in the same hub files.** Input handler in 46 of 81 post-pivot commits; one 856-line match. Friction · L. (intent_handler.rs:62-917, sessions/state.rs:31)
13. **F-13 The riskiest code has no tests; existing tests fight refactors.** Loop untested; worker tests skip the mailbox; id-blind fixtures. Risk + friction · M. (run.rs:508-1561, intent_handler.rs:2063-2068)
14. **F-14 Layout logic duplicated for floats and layered by rewrites.** Friction · M. (tree.rs:1063-1091, layout/state.rs:398-483 and 744-845)
15. **F-15 Each worker command is declared three times.** 33 commands, 27 forwarded one-to-one. Friction · S–M. (run.rs:850-1107)
16. **F-16 One-session leftovers in schema and code.** Dead tables/columns, v11 machinery on every start, new sessions saved through the migration helper, "thread" vocabulary. Hygiene · S–M. (store.rs:501-517 and 967, orb-domain/Cargo.toml:19)
17. **F-17 Project docs describe an app that no longer exists.** RECORD.md about a third false; AGENTS.md §3 describes a bus that isn't there. Hygiene · S–M. (.agents/RECORD.md:66-67, AGENTS.md:223-245)

Backlog bugs (not architecture): B-1 sidebar search ignores pane names; B-2 seen marks follow hover; B-3 the shown session can auto-settle; B-4 a finished restore/move only shows if the cursor stayed; B-5 moving up into a stack picks the last pane; B-6 new session's first tab saved with the wrong swap layout; B-7 start-screen "exited at start" names no session; B-8 `claude agents` polls with no Claude pane; B-9 add-project/branch switch hold the state lock during I/O. Details in `falsified.md`. B-5 dropped (unconfirmed; recheck during layout work). B-10 if the layout tables cannot be read at all, the saved jump list is filtered to nothing and may be saved back empty (found by the data-safety fix).

User answers at checkpoint 1: **B-2** the "new reply" mark clears when the user moves into that agent's pane, not on hover. **B-3** remove auto-settle entirely; keep worktree auto-prune (prune still waits for settled or deleted sessions). **B-4** a finished restore, move, branch switch or new session shows if it's still the active session on the right, even if the cursor moved. These reverse parts of decisions.md "Settling"; record there when built.

## Reference patterns

From jinn, herdr and zellij: ~90 pattern cards in `.plans/architecture-overhaul/scouts/` (jinn-actors, jinn-observability, herdr-panes, herdr-ops, zellij-threads, zellij-messaging, zellij-layout, zellij-robustness). Each accepted decision below names the cards it borrows.

## Target architecture

Accepted at checkpoint 2 (2026-10-11). Full reasoning, evidence and rejected alternatives: `.plans/architecture-overhaul/target.md` and `target-falsified.md` (the falsified versions are what was accepted). Pattern cards: `scouts/*.md`.

- **D-16 Docs match the code.** Fixes F-17. RECORD.md and AGENTS.md §3 describe today's orb; a script checks that names they mention exist (allowlist for AGENTS.md's illustrative examples like `ActorHost`). RECORD edits need the user's OK. Re-run after D-7 and D-12.
- **D-10 One log file.** Fixes F-10, B-7. Hand-rolled append-and-roll file under `~/.orb` (no new crate); full error cause chain for every error, one line per kill, prune and deletion. Log path injected (tests use a temp path). Rule: no `let _ =` on kill/remove results. Borrows HERDR-OPS-11, ZJ-ROB-5.
- **D-8 A crash ends orb cleanly.** Fixes F-5. Panic hook logs (D-10), restores the terminal, exits; panes survive in zmx. **User's answer:** a panic inside one pane's output handling drops only that pane's connection with a message; any other panic ends orb. Borrows ZJ-ROB-1/2, JINN-ACT-7.
- **D-2 Copy the database before every upgrade.** Fixes F-2. Copy aside whenever the stored version is behind; keep 3; the "newer schema" refusal names the copy. **User's answer:** a branch/test build refuses to upgrade the live database unless explicitly asked. Extends "Migration of live data". Borrows HERDR-OPS-3.
- **D-14 Tests at the seams (absorbs D-1).** Fixes F-13, guards F-1. Actor tests through the real mailbox; distinct id ranges per kind in new fixtures (old tests converted only when touched); a "no pane lost" helper on every tree-op and layout-save test; loop decisions as plain functions; a fake pane connection for loop tests (**approved by the user**). Grep: a `zmx list` error never feeds a close or kill. Borrows JINN-OBS-11, HERDR-PANE-12.
- **D-4a Time limits on every outside program.** Fixes F-4/F-8/F-11 hang halves, B-8. git's runner and `du` go through the existing timed runner (child.rs); `claude agents` skipped when no Claude pane exists. Rule: `rg '\.output\(\)'` in production only in the runner. **User's note:** retrying a timed-out call is welcome if simple; exponential backoff is likely overkill (retry read-only calls once at most).
- **D-5 The screen thread waits less.** Fixes F-8. Timed runner polls fast at first (fast command returns in <5 ms); the branch picker's 4 git calls run in parallel; session-picker preview re-read at most once a second.
- **D-6 Draw first, then reconnect panes.** Fixes F-9 (start-up half). Hidden-pane redraw skipping left out until CPU is measured.
- **D-9 A real quit.** Fixes F-11. Quit stops actors gracefully so queued saves land (2 s cap), store lock released after; SIGTERM/SIGINT/SIGHUP take the same path. Borrows JINN-ACT-11, HERDR-OPS-8/9.
- **D-12 One safe pane-kill path; no auto-settle.** Fixes F-6, a found race (settle kill during a failed zmx list re-creates a bare shell), B-3. The sessions actor's kill-panes removes the session from the shown set itself before killing; the loop never re-attaches a pane whose session isn't shown; auto-settle removed (user's checkpoint-1 call; record in decisions.md "Settling"). Loop keeps its exit verdict.
- **D-3 One record per pane.** Fixes F-3, B-1. Delete the sessions actor's private pane map; unread panes' rows live in the unread record (test first: a partial load keeps their names through a save); store rows built from layouts at save; agent rows show only panes a layout holds; a pane's report file is deleted with its row; search matches the shown name. Test: names equal across sidebar, picker, search.
- **D-11 Delete one-session leftovers.** Fixes F-16, B-6. Dead functions, unused `unicode-normalization`, the start-up `--bg` stop step, new sessions saved through the normal path. **User's answer:** also drop the dead schema columns and tables with a migration (after D-2 and D-3).
- **D-7 in: one command enum per actor.** Fixes F-15. Frontend forwards with one `tell` per actor instead of a 33-arm switch. Borrows ZJ-MSG-1.
- **D-4b Slow jobs beside the sessions queue.** Fixes F-4. New-worktree creation and the poll's outside calls run beside the sessions actor and post results back, re-checked on arrival. Branch switch and add project stay inline but stop holding the state lock (B-9). **Reopens "Git plumbing"** (approved).
- **D-7 out: Design A, a typed report channel.** Fixes F-7, B-4. Workers send typed reports on the loop's channel; one loop function applies them; attach slot, notice queue, faked relist intent and toast text-diff go; workers never write the picker. Build from spike A (8a56291) but keep today's behaviour for its three drifts, each with a test: a repeating poll save error doesn't re-pop every poll; the projects screen keeps re-sorting live; an error is not hidden by a notice arriving together. Contradicts AGENTS.md §3 "no feedback loop" — update §3 (D-16). Changes "Notifications (M9)" mechanism.
- **D-15 Layout: one close rule, drawn stack geometry.** Fixes F-14, B-5 if reproduced (test first). Tiles/floats merge left out.
- **D-13 Split hub files.** Fixes F-12. Ids into one shared module; intent match becomes one-line calls into per-feature files; split the sessions actor by concern only if still >2,000 production lines; ratchet script on file length. **User's answer:** also rename "thread" → "agent" in code and on screen. Do it in a quiet window right after an ff-merge, with no feature branch open.

## Migration steps

Each step: tests first → change → `just test` + `just lint` → RECORD facts → commit → checkpoint 3. Behaviour unchanged unless noted.

1. **D-16** docs match today.
2. **D-10** log file.
3. **D-8** crash ends orb (per-pane exception).
4. **D-2** copy before every upgrade; branch builds ask before upgrading.
5. **D-14** test seams + "no pane lost" helper + zmx-list grep (tests only).
6. **D-4a** time limits; skip `claude agents` with no Claude pane.
7. **D-5** screen thread waits less.
8. **D-6** draw first.
9. **D-9** real quit and signals.
10. **D-12** kill path guard, no re-attach for hidden sessions, remove auto-settle (behaviour change; update decisions.md "Settling").
11. **D-3** one pane record (+B-1).
12. **D-11** delete leftovers (+B-6), then drop dead columns/tables by migration.
13. **D-7 in** one command enum per actor.
14. **D-4b** slow jobs beside the queue (update decisions.md "Git plumbing").
15. **D-7 out** report channel, Design A (+B-4).
16. **D-15** close rule and stack geometry (+B-5).
17. **D-13** split hub files + thread→agent rename, in a quiet window.
18. **D-16** final docs pass.

Still in the backlog after the plan: B-11 a suspended (stopped, state T) old `claude` in the same pane keeps reporting "busy" in `claude agents --json`, so the agent row shows working and "1 running" counts it; ignore stopped processes or prefer the pane's foreground process (seen 2026-10-11: pids 49709 stopped + 28162 live, same session id). B-2 (the "new reply" mark clears when the user moves into the agent's pane), B-10 (jump list may save back empty when layouts can't be read).

## Drift log

Where the implementation left the target, and why.

- **Step 2 (D-10), 2026-10-11:** "full error cause chain for every error" is narrowed. Reads that are retried and remove nothing still drop their error: actor mailbox sends, transcript scans, the branch read after a turn, worktree fact reads, the session picker's preview read and desktop notifications. D-7's report channel (plan `overhaul-4-messaging`) revisits them.
