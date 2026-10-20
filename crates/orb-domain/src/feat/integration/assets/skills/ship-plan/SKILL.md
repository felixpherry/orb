---
name: ship-plan
user-invocable: false
description: Takes an approved plan from implementation to an open pull request with nobody watching. Runs the chosen task loop on autopilot, taste tests the build like a user, reviews it, has a falsifier argue against each finding, fixes what's proven in one round, simplifies, checks the work against the plan, taste tests again, opens a PR, gets its CI green, and reports back to whoever started it. Never merges. Use when approve-plan or hand-off hands over a plan to finish unattended, or when the user says "ship this plan".
argument-hint: "<absolute path to .plans/task> <micro|simple|phased> [report-to <orb pane id>]"
---

# Ship plan

You run unattended: nobody answers questions. Never stop to ask. When something needs a decision, take the option closest to the plan's intent and keep going. If that changes something the user approved (what the feature does, its scope, an acceptance criterion, a Record entry), add a divergence note to `plan.md` saying what you did instead and why. Stop early only when an unrecoverable error blocks all progress; then go straight to **Report back**.

**Arguments:** the plan folder as an absolute path (a fresh worktree has no `.plans/`), the loop to run, and optionally the orb pane to report to.

**Never:** merge, force-push, push to the main line, or touch files outside the repo.

## You orchestrate, helpers do the work

Your context is for decisions, not file contents. Run every step below in a fresh subagent and keep only what it reports back. Use subagents, not orb sessions or colonists: helpers must work in this worktree, and a new session would get a different checkout.

- **Each helper gets** the plan folder, the step's skill and arguments, the unattended rule above (never ask; note divergences in `plan.md` and risks in `risks.md`), and what to return: at most ten lines on what it did, what it changed, what failed, and what's left open.
- **You read only** those reports, `tasks.md`, and `git status`. Don't open source files, diffs or test logs yourself. If a report is unclear, send a helper to find out.
- **One helper at a time,** since they share the worktree. The only exception is the review step's reviewers, which only read.
- **A helper that has reported is done.** If it's still running (in the background, in a pane, in a session you started), stop it.
- **Helpers may start their own helpers** when their skill says so, like interrogate's reviewers or the phased loop's per-phase agents.
- **If you have no subagent tool,** run the step yourself.

## Pipeline

Run the steps in order. Each step names the skill it uses, so changing this list changes the pipeline.

1.  **Build.** A helper runs the chosen loop: `/run-<loop>-loop <plan folder>`. For the phased loop, the helper is that loop's orchestrator, so don't tell it to run solo.
2.  **Taste test.** A helper runs taste-test on the plan folder. Its broke lines go to the fixer. Its couldn't-try lines go to **Known limitations**. Any change it made to the recipe goes in your report back, not the PR.
3.  **Review.** Helpers at the same time, read-only:
    - interrogate;
    - check-blast-radius on the branch's diff;
    - the repo's own review skill or agent, if it has one.

    Tell each reviewer to skip style (lint and deslop cover it) and to back every finding with proof: a real run that shows it, or a test that fails now and would pass with the fix. Merge their reports and drop duplicates. The must-fix findings are interrogate's **Act on**, a blast-radius risk it confirmed, and a broken written rule of the repo.
4.  **Falsify.** A helper runs falsify on the must-fix findings, with the plan folder and the branch's diff. No proof, no fix: an unproven finding is left open or dismissed.
5.  **Fix, once.** One helper does what falsify ruled (cut, narrow, fix) and fixes taste-test's broke lines. Each fix comes with a test that fails without it, where a test can show it. A fix that needs a new rule or special case (a branch for one input, a new setting or flag) isn't made: it goes to **Known limitations**, since it often means the design is wrong. There is no second fix round.
6.  **Simplify.** A helper runs the `simplify` skill if you have it, then deslop, on the branch's changes, then runs `test`.
7.  **Spec check.** A helper runs check-gaps on the plan folder, fixes every gap it lists, then runs `check` and `test` until both pass.
8.  **Verify.** A helper runs `check` and `test`, then lints and formats the files that changed, and fixes until everything passes.
9.  **Taste test again.** A helper runs taste-test's second run. Anything still broken goes to **Known limitations**; don't fix it.
10. **Tidy the branch.** A helper runs make-pr-easy-to-review before anything is pushed: it squashes noise into clean commits, writes the PR description (see below) to a file, and lists the pictures to attach. Give it the plan folder.
11. **Open the PR.** A helper pushes the branch and opens the PR with that description, using the repo's own PR skill if it has one, else `gh pr create`. It attaches the listed pictures as make-pr-easy-to-review says, with `gh pr edit` after if the repo's skill can't attach. It returns the link. Then stop every helper still running.
12. **Babysit CI.** A helper runs babysit with `ci-only` on the new PR: it fixes failing checks and merge conflicts, then stops. It never waits for reviewers.
13. **Report back.** You do this one.

## The PR description

Short, so the user reads all of it. Teammates review it too, so it only points at what they can open: the diff, the ticket, linked PRs. Never the plan folder, the recipe, or a path on this machine. Put these at the top, before the repo's usual template:

- **Summary:** what changed, at most 5 bullets.
- **How it was tried:** what taste-test did and saw, in a few lines, with the pictures make-pr-easy-to-review picked. Script output and logs go in as a table or code block, never as a picture of text. Commands to try it only if they run from the repo itself.
- **Known limitations:** stated as facts for a reviewer, worst first, not as asks. Anything still broken after the second taste test, checks it couldn't try, fixes held back because they need a new rule, divergences, and open findings worth a look. Write each one out; don't send readers elsewhere for the rest.

Everything else stays in the plan folder: every divergence note, open and dismissed finding, and the risk list.

## Report back

One line: `<plan name>: PR <link>. CI <green|red>. <N> known limitations.` (or what stopped you, if you stopped early).

- **Put the PR on the user's plate (in orb only).** When a PR was opened and `$ORB_PANE_ID` is set, run `orb plate add "Review PR #<n>: <title>" --verb review --link <PR url>` before sending the line. If orb refuses with `colonist`, you run in a colony: skip it, since your report back covers it. With no PR (you stopped early), add nothing.
- **With a `report-to` pane:** send it with `orb agent prompt <pane> "<line>"`. That agent tells the user and updates their status board.
- **Without one:** end your final message with it.
