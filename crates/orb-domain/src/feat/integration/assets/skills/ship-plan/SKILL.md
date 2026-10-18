---
name: ship-plan
user-invocable: false
description: Takes an approved plan from implementation to an open pull request with nobody watching. Runs the chosen task loop on autopilot, checks the work against the plan, simplifies it, reviews it from several angles, has a falsifier argue against each finding, fixes what survives, opens a PR, gets its CI green, and reports back to whoever started it. Never merges. Use when approve-plan or hand-off hands over a plan to finish unattended, or when the user says "ship this plan".
argument-hint: "<absolute path to .plans/task> <micro|simple|phased> [report-to <orb pane id>]"
---

# Ship plan

You run unattended: nobody answers questions. Never stop to ask. When something needs a decision, take the option closest to the plan's intent, add a divergence note to `plan.md` saying what you did instead and why, and keep going. Stop early only when an unrecoverable error blocks all progress; then go straight to **Report back**.

**Arguments:** the plan folder as an absolute path (a fresh worktree has no `.plans/`), the loop to run, and optionally the orb pane to report to.

**Never:** merge, force-push, push to the main line, or touch files outside the repo.

## You orchestrate, helpers do the work

Your context is for decisions, not file contents. Run every step below in a fresh subagent and keep only what it reports back. Use subagents, not orb sessions or colonists: helpers must work in this worktree, and a new session would get a different checkout.

- **Each helper gets** the plan folder, the step's skill and arguments, the unattended rule above (never ask; note divergences in `plan.md`), and what to return: at most ten lines on what it did, what it changed, what failed, and what's left open.
- **You read only** those reports, `tasks.md`, and `git status`. Don't open source files, diffs or test logs yourself. If a report is unclear, send a helper to find out.
- **One helper at a time,** since they share the worktree. The only exception is the review step's three reviewers, which only read.
- **Helpers may start their own helpers** when their skill says so, like interrogate's reviewers or the phased loop's per-phase agents.
- **If you have no subagent tool,** run the step yourself.

## Pipeline

Run the steps in order. Each step names the skill it uses, so changing this list changes the pipeline.

1.  **Build.** A helper runs the chosen loop: `/run-<loop>-loop <plan folder>`. For the phased loop, the helper is that loop's orchestrator, so don't tell it to run solo.
2.  **Spec check.** A helper runs check-gaps on the plan folder, fixes every gap it lists, then runs `check` and `test` until both pass.
3.  **Simplify.** A helper runs the `simplify` skill if you have it, then deslop, on the branch's changes, then runs `test`.
4.  **Review.** Three helpers at the same time, read-only:
    - the repo's own review skill or agent if it has one, else review-code with `standards-only`;
    - interrogate, so four reviewers check it from different angles;
    - check-blast-radius on the branch's diff.

    Merge their reports and drop duplicates. The must-fix findings are interrogate's **Act on**, a blast-radius risk it confirmed, and a broken standard. Keep the rest for the PR.
5.  **Falsify.** A helper runs falsify on the must-fix findings, with the plan folder and the branch's diff. Then one helper does what it ruled: cut, narrow, fix or revert. Its leave-open and dismissed lines go to the PR.
6.  **Verify.** A helper runs `check` and `test`, then lints and formats the files that changed, and fixes until everything passes.
7.  **One more round.** If step 5 changed code, run steps 3 to 6 again on just those changes. In this round falsify also rules on bugs the first round's changes caused. After this round, stop fixing and carry what's left to the PR.
8.  **Tidy the branch.** A helper runs make-pr-easy-to-review before anything is pushed: it squashes noise into clean commits and writes the PR description (see below) to a file.
9.  **Open the PR.** A helper pushes the branch and opens the PR with that description, using the repo's own PR skill if it has one, else `gh pr create`. It returns the link.
10. **Babysit CI.** A helper runs babysit with `ci-only` on the new PR: it fixes failing checks and merge conflicts, then stops. It never waits for reviewers.
11. **Report back.** You do this one.

## The PR description

Put these at the top, before the repo's usual template:

- **Divergences:** every place the build differs from the plan, and why, from the divergence notes in `plan.md`.
- **Open findings:** review findings you didn't fix, one line each, with who raised them and falsify's reason for leaving them.
- **How to try it:** the commands to run or the pages to open.

## Report back

One line: `<plan name>: PR <link>. CI <green|red>. <N> divergences, <M> open findings.` (or what stopped you, if you stopped early).

- **Put the PR on the user's plate (in orb only).** When a PR was opened and `$ORB_PANE_ID` is set, run `orb plate add "Review PR #<n>: <title>" --verb review --link <PR url>` before sending the line. If orb refuses with `colonist`, you run in a colony: skip it, since your report back covers it. With no PR (you stopped early), add nothing.
- **With a `report-to` pane:** send it with `orb agent prompt <pane> "<line>"`. That agent tells the user and updates their status board.
- **Without one:** end your final message with it.
