---
name: fix-ci
description: Finds failing PR checks, reads their logs or check links, and applies focused fixes until the checks pass. Use when a branch or PR has red CI, or when babysit hands over failing checks.
metadata:
  based-on: "fix-ci from pstack by Lauren Tan, via michael-denyer/pstack-claude (MIT)"
---

# Fix CI

## Trigger

Branch or PR CI is failing and needs a fast, iterative path to green checks.

## Workflow

1. Resolve the active PR and inspect `gh pr checks --json name,bucket,state,workflow,link`.
2. Inspect failed jobs and extract the first actionable error. Use GitHub Actions logs (`gh run view <run-id> --log-failed`) when available; otherwise use the check link to identify the failing command or service.
3. Apply the smallest safe fix.
4. Push, re-check the PR check set, and repeat until green.

## Guardrails

- Fix one actionable failure at a time.
- Prefer minimal, low-risk changes before broader refactors.
- Keep `gh pr checks` as the source of truth for overall PR CI state.
- Never skip hooks, mark a check as not required, or loosen a test's expected values just to go green.
- If the fix needs a design choice, stop and report it instead of guessing.

## Output

- Primary failing job and root error
- Fixes applied in iteration order
- Current CI status and next action
