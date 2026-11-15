# Record entry format

Applies to every proposed entry when `.agents/RECORD.md` is missing. When the file exists, its own format rules govern and these are the floor.

- **Factual, present tense.** Entries assert how the application works _now_, written as they will read immediately after the approved implementation. Any entry containing "will", "should", "later", "future", "reserved", "roadmap", or a version milestone ("v1", "v0.2") is plan-speak: rewrite it as a present-tense fact or drop that half.
- **Single tag.** Each entry is a markdown list line beginning with exactly one subsystem tag: `- (video) ...`. Tags are lowercase, short, subsystem-shaped. If two tags seem necessary, split the entry or re-scope it. For a new project, invent tags freely.
- **Single concept, one sentence.** If a semicolon would be needed to join two facts, write two entries.
- **Template-shaped.** `[Scope] currently [does X / is Y].` · `[Scope] persists [what] to [where].` · `[Input/event] is handled by [actor/subsystem], which [action].` · `[Scope] is bounded by [constraint].`
- **Durable, not task-shaped.** Tasks, goals, TODOs, and version milestones are never entries.
- **On-disk form.** Entries are plain markdown list lines: no surrounding quotes, no rationale, no prose wrapper.
- **Application facts, not environment facts.** Record rules of the application ("camera sources are configured by by-id paths"), not descriptions of the authoring machine's hardware.
