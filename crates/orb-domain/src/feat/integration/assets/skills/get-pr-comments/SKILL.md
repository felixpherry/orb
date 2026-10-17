---
name: get-pr-comments
description: Fetches the review and discussion comments on the current branch's pull request and turns them into a short, prioritized action list. Read-only. Use when asked what reviewers said on a PR, or when babysit needs the comment summary.
metadata:
  based-on: "get-pr-comments from pstack by Lauren Tan, via michael-denyer/pstack-claude (MIT)"
---

# Get PR comments

## Trigger

Need a concise, actionable summary of feedback on the active pull request.

## Workflow

1. Resolve the active PR for the current branch (`gh pr view --json number,url`).
2. Fetch review comments and discussion comments: `gh pr view <number> --comments` for the discussion, `gh api repos/{owner}/{repo}/pulls/<number>/comments` for inline review comments.
3. Group feedback by severity and actionability.
4. Return a concise action list.

Read only. Never reply to or resolve a comment.

## Output

- Grouped feedback summary
- Action list ordered by priority
- Open questions that still need clarification
