# Research

This folder is one research question. One session orchestrates it: it plans,
hands focused tasks to subagents, keeps the shared record in `state.md` and
the data sources in `SOURCES.md`, and answers only when the evidence is
enough.

## Which part of this file is yours

- **You were dispatched for a gap** (your prompt names a gap ID like `G3`):
  follow only "Subagents" below. Don't dispatch agents and don't edit
  `state.md` or `SOURCES.md`.
- **Otherwise you're the orchestrator.** Follow "Orchestrator". If `state.md`
  says `Status: running` and you didn't start that round in this session,
  another thread may be orchestrating: ask the user before you write anything.

## The record

Five kinds of entry, each with an ID. Never call anything a "fact".

- **Evidence (E)**: an observation, with the source it came from and where in
  that source. "The response was 212 KB" is evidence; "payloads are small" is
  not. Evidence made by an analysis says which one (`from: A1`).
- **Analysis (A)**: a transformation of evidence: a script, a simulation, a
  query, a calculation. It names its inputs, method, parameters and
  assumptions. Its outputs are new evidence.
- **Claim (C)**: a statement the cited evidence supports, no wider than that
  evidence. Its `scope` says what it covers.
- **Hypothesis (H)**: anything wider than the evidence: a prediction, a
  generalisation, an explanation not yet tested. It names what it came from,
  what it predicts, and the gaps that would test it.
- **Gap (G)**: something we need but don't have. A contradiction between two
  entries is a gap too.

The scope rule: if a statement goes beyond what its evidence covers (another
dataset, production instead of staging, the future), it's a hypothesis, not a
claim.

`state.md` looks like this:

```markdown
# <question>

Status: running | paused | done
Round: 2

## Success criteria
- S1: <what the answer must establish> — met by: C1 (or: unmet)

## Evidence
- E1: 30 days of production requests, 4.1M rows.
  source: prod-requests. by: user. at: 2026-09-20
- E2: 15m TTL gives a 66% hit rate. from: A1. at: 2026-09-21
- E5: Cache entries are written with a fixed 15m TTL.
  source: app src/cache.rs:42. by: collector (G2). at: 2026-09-21

## Analyses
- A1: TTL simulation. inputs: E1. method: analyses/ttl_sim.py.
  params: ttl = 15m, 30m, 60m. assumptions: request mix is stationary;
  cache has no size limit. outputs: E2, E3, E4

## Claims
- C1: Higher TTL raised the simulated hit rate on this dataset.
  scope: E1's 30 days, simulated. evidence: E2, E3, E4. verdict: holds
  critique: <what the critic said>

## Hypotheses
- H1: Raising the production TTL should give a similar gain.
  from: C1. predicts: hit rate rises after the change. tested by: G4, G5

## Gaps
- G4: [research] known: C1 assumes no cache size limit (A1).
  unknown: the production cache's size limit and eviction policy.
  method: read the cache config in the repo and its deploy values.
  done when: a limit and policy are cited from config. status: open
- G5: [user] known: … unknown: … ask: <the exact thing to ask the user>.
  status: needs_user
```

Big raw data goes in `evidence/`, scripts in `analyses/`; `state.md` points at
them by path.

### Sources

Every place real data comes from is listed once in `SOURCES.md`, under a
short name. Evidence cites that name plus where in it (a file and line, a
query, a time range). A source says what it is, where it is, which version,
how to get at it, and whether it may be changed:

```markdown
# Sources

- app: the service's repo. ~/dev/app at a1b2c3d (main). read-only.
- prod-requests: 30 days of production request logs, exported by the user
  on 2026-09-20 with `logs export --since 30d`. evidence/requests.csv.
- staging-api: https://staging.example.com, queried with curl. Staging data,
  not production.
```

Pin a repo to its commit. If it moves on, add a new source for the new
commit instead of editing the old one, so earlier evidence still points at
the code it was read from. Never change a repo or system that's a source:
experiment on a copy in `analyses/`.

## Orchestrator

You manage uncertainty; subagents do the investigating. Read `state.md` and
the files it points at, and hand everything else to a subagent.

1. **Start or resume.** If `state.md` exists, read it and continue from its
   open gaps. Never start over. If it doesn't, write the question, the success
   criteria (what a finished answer must establish), and `Status: running`,
   and list in `SOURCES.md` every source the question names (a repo with its
   current commit, a system, a file). If the question is ambiguous, ask the
   user before planning.
2. **Find the gaps.** What is unknown, and what evidence would settle it?
   Write each as a gap.
3. **Dispatch.** Hand open gaps to subagents, at most 3 at a time, in
   parallel when they don't depend on each other. Each prompt contains: the
   gap's fields, the IDs it builds on, the paths to `state.md` and
   `SOURCES.md`, the matching role section from "Subagents" (copied in
   full), and "Return" below.
   Mark the gap `running`.
4. **Merge.** You are the only writer of `state.md` and `SOURCES.md`. Give
   new entries the next free IDs and rewrite their references, and add the
   sources they return. Reject or fix before writing:
   - evidence whose source isn't in `SOURCES.md` or returned with it, evidence
     that doesn't say where in the source, or an interpretation posing as
     evidence;
   - a claim that cites no evidence, or is wider than its evidence (make it
     a hypothesis);
   - an analysis with no assumptions listed;
   - a gap without `known`, `unknown`, `method` and `done when`.
5. **Critique.** Dispatch the critic on `state.md` and `SOURCES.md` alone
   (no transcripts).
   Record its verdict on each claim, and add the gaps it finds.
6. **Decide.**
   - Open research gaps left: go to 3 and bump `Round`.
   - Only user gaps left: pause.
   - Every criterion met by a claim with `verdict: holds`: synthesise.
   - After 5 rounds without that, pause and ask the user whether to go on.

A follow-up must be targeted. Not "research this further", but: "We know
payload growth happens on `/products/:id` (E3). We don't know which field
causes it. Take the responses in `evidence/` and compute each top-level
field's serialised size."

### Pausing for the user

Missing real-world data is a normal state, not a failure. Never fill it by
guessing, and never let smaller or staging data stand in for what the
question is about.

Set `Status: paused`, save `state.md`, then ask the user for exactly what
each `needs_user` gap needs, why, and in what form (a file to put in
`evidence/`, a command to run, a value). Then stop.

When the user answers, add what they gave to `SOURCES.md` (what it is, how
they got it, when) and save it as evidence (`by: user`), close the gap, set
`Status: running`, and continue from step 4.

### Synthesis

Write `answer.md`, and give the same answer in the chat:

- the answer to each success criterion, citing claim IDs;
- hypotheses, labelled as hypotheses, with what would test them;
- what is still unknown, and why.

State only claims the critic said hold. Then set `Status: done`.

## Subagents

Do only your gap. Don't edit `state.md` or `SOURCES.md`. Put raw output in
`evidence/<gap>-<name>` and scripts in `analyses/<gap>-<name>`. Never change a
source: experiment on a copy. Never guess to fill missing data: report it as
a user gap.

### Collector

Get evidence: read code, logs, files and docs; run commands, queries and
experiments. Report what you observed, which source and where in it, and
when, quoting output or pointing at the file you saved. A source not yet in
`SOURCES.md` goes in your return, described the same way. Don't conclude what
it means. If what you can
reach isn't representative of what the gap asks about, say so and return
`needs_user_input`.

### Investigator

Reason over the evidence in `state.md` and the files it points at. Propose
claims (cited, within their evidence's scope), hypotheses (with predictions
and how to test them), and the gaps that would settle them. Run analyses when
they help, and record their inputs, method, parameters and assumptions.

### Critic

Attack the record; don't propose an answer. For each claim, return a verdict
(`holds`, `weak` or `refuted`) and why. Look for:

- claims with weak or missing evidence, or wider than their evidence;
- the same unstated assumption in several entries;
- evidence that contradicts other evidence or a claim;
- analysis assumptions that don't hold;
- generalising from too little data, or from a source that isn't what the
  question is about (staging for production, a sample for the whole);
- what would falsify the leading hypothesis, and whether anyone has looked;
- references to IDs that don't exist, and gaps too vague to act on.

Every problem you find that more work could settle becomes a gap.

### Return

End with this block, using `new-E1`, `new-C1`, … as IDs:

```markdown
status: complete | needs_more_research | needs_user_input

sources:
evidence:
analyses:
claims:
hypotheses:
verdicts:
gaps:
```

- `complete`: the gap is settled by what you return.
- `needs_more_research`: you made progress; the gaps you return can be done
  without the user.
- `needs_user_input`: the evidence needed isn't reachable from here; your
  gaps say exactly what to ask the user for.
