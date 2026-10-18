---
name: babysit
description: Watches an open PR and drives it to a mergeable state by fixing failing CI, resolving merge conflicts, and applying the straightforward review comments. Never replies to reviewers or merges. Use after opening a PR when the user wants it kept green without re-prompting, or when they say "babysit this PR".
metadata:
  based-on: "babysit from pstack by Lauren Tan, via michael-denyer/pstack-claude (MIT)"
---

# Babysit a PR

A loop over the `gh` CLI: check the PR, fix what's broken, push, check again.

## When to use

- There's an open PR and the user wants it kept green.
- The user invokes `/babysit` directly.
- A subagent that opens a PR does NOT babysit. It returns to the parent and lets the parent decide.

## Steps

If the arguments say `ci-only`, skip review comments and don't wait for reviewers: stop as soon as the checks settle, or at the three-round limit.

1. **Fetch PR state.**

   ```bash
   gh pr view <number> --json number,title,state,mergeable,reviewDecision,statusCheckRollup,mergeStateStatus,comments,reviews
   ```

2. **Triage in priority order.**
   - Merge conflicts (`mergeStateStatus == DIRTY`): run the **fix-merge-conflicts** skill. Merge the base branch into the PR branch rather than rebasing, so a normal push works and no force-push is needed.
   - Failing checks (`statusCheckRollup` entries with `conclusion: FAILURE`): run the **fix-ci** skill. Root-cause the failure; fix the underlying code or test; commit; push.
   - Review comments: run the **get-pr-comments** skill for the summary, then act only on feedback you actually agree with. When a comment has a single mechanical answer (a rename, a guard clause, a formatting nit), make the edit and quote the comment in the commit message. When it hinges on a judgement call, or you can't tell what's being asked, don't guess: leave it and put it in your report with what you would have done.
   - Review-bot comments (Bugbot and similar automation): decide fix or dismiss before acting. Security, data-loss, and high-severity findings go in the report for the user to decide; don't apply or dismiss them yourself.

3. **Loop.** Pick the wait from what you're watching:
   - Active CI run: `gh pr checks <number> --watch` blocks until the checks finish, so no separate interval is needed.
   - Awaiting a reviewer: if the harness can schedule a re-check (Claude Code's `loop` skill), use a 20 to 30 minute heartbeat. Otherwise stop once checks settle and report.

4. **When to stop.**
   - Build is green, the branch merges cleanly, and every comment is either applied or listed in the report → call it ready.
   - Three rounds of fix → push → recheck and it still isn't fully green → stop and summarise what's still broken.
   - The next fix would force a design choice → stop and put the choice in the report. Don't ask mid-run.

5. **Report** to the user, not on the PR: fixes applied, comments applied, comments skipped or disagreed with (each with its reason and what you'd do), current PR status. Cite each commit by SHA.

## Hard rules

- **Never reply to reviewers.** No PR comments, no review replies, no resolving threads with a message. Talking to teammates is the user's job; everything they need goes in your report.
- Never merge. Never force-push or rebase a branch others may have pulled. If one looks necessary, stop and report it.
- Don't tweak a test's expected values just to get a pass. Only change an assertion when the behaviour genuinely changed and the assertion was pinned to the old behaviour.
- Never skip hooks (`--no-verify`).
- Never bypass a failing check by marking it as not required.
- Never mark a draft PR ready: that notifies reviewers. Say in the report that it's ready and let the user flip it.

## Cross-refs

- Use `interrogate` before opening if the diff is contested; once open, babysit takes over.
- Use `unslop`, if you have it, on any prose you write here (commit messages, the report).

## Provenance

This is a Claude Code analog of Cursor's `/babysit`, not a port: Cursor's implementation is closed source. The skill is independently authored; the workflow is informed by Cursor's public `/babysit` behavior.
