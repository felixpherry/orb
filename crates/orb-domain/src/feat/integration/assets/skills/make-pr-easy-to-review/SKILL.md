---
name: make-pr-easy-to-review
description: Prepares a PR for review by cleaning noisy commit history and rewriting the PR description with reviewer guidance, without changing code behavior. Use for "make this easy to review", "tidy this PR", or "clean up commits".
metadata:
  based-on: "make-pr-easy-to-review from pstack by Lauren Tan, via michael-denyer/pstack-claude (MIT)"
---

# Make PR Easy to Review

Prepare a PR so a reviewer can quickly understand the intent, important files, and risk. The default goal is reviewability without behavior changes.

## Workflow

1. Resolve the target PR from the user-provided URL or current branch.
2. Inspect commits, diff size, changed paths, generated files, and PR description.
3. Identify reviewability issues: noisy commits, stale description, unrelated changes, mixed mechanical and logic changes, missing tests, or unclear reviewer entry points.
4. Apply safe improvements, then verify the tree or diff still matches the intended code.
5. Report what you changed, including whether you rewrote history.

## Before the PR exists

When the branch has no PR and hasn't been pushed (ship-plan calls it this way), every commit is local and yours, so clean up history without being asked. Note `git rev-parse HEAD^{tree}` before you start and check it's unchanged after. Don't push, and write the description to the file the caller names instead of running `gh pr edit`.

## History Cleanup

Rewrite history only when the user asked for commit cleanup and every commit on the branch is theirs (check `git log --format='%an' origin/<baseRefName>..origin/<headRefName>`). If someone else has pushed to the branch, leave history alone and say so in the report. Before rewriting:

```bash
gh pr view <PR> --json title,headRefName,baseRefName,state,commits
git fetch origin <headRefName> <baseRefName>
ORIGINAL_TREE=$(git rev-parse origin/<headRefName>^{tree})
```

Good commit groupings usually follow dependency order:

1. Schema/storage or generated API definitions.
2. Core logic.
3. Wiring and integration.
4. UI or surface behavior.
5. Tests.

After rewriting, verify content identity:

```bash
echo "Original tree: $ORIGINAL_TREE"
echo "Current tree:  $(git rev-parse HEAD^{tree})"
git diff origin/<headRefName> --stat
```

Do not push if the tree changed unintentionally. If it matches, push with `git push --force-with-lease` (never plain `--force`) and say in the report that history was rewritten.

## Reviewer Guidance

Guidance goes in the PR description (`gh pr edit <PR> --body-file <file>`), never in comments on the diff. Keep it short:

- Add a TL;DR that matches the actual diff.
- Separate core files from generated or mechanical files.
- Call out risky behavior changes, migration order, rollout plan, and test coverage.
- Link issue trackers, dashboards, or design docs when they explain intent.

## Guardrails

- Never hide meaningful behavior changes inside "cleanup".
- Never post PR comments or review annotations. The description is the only place you write.
- Do not bypass hooks unless the user explicitly asks.
- If the PR is too large to make reviewable with notes, recommend splitting instead of polishing around the problem.
