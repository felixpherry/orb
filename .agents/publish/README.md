# Publishing the public repo

publish.py builds github.com/zqccer/orb from a branch of this private repo. It clones the branch fresh and rewrites its history in one git filter-repo run:

- **Author:** every commit becomes zqccer with the account's GitHub noreply email.
- **Messages:** old commits get their message from messages.jsonl (one JSON row per commit: id and message). Merges read `Merge branch 'main' into <topic>`.
- **Paths:** `.agents/`, the top-level `.claude/`, and `docs/` except `docs/demo/`, `docs/dev/` and `docs/keys.md` are gone from every commit. README.md, CHANGELOG.md and the research kit's `.claude/` stay.
- **Text:** home paths and the old user name are replaced; the private block in AGENTS.md is cut; AGENTS.md versions from before the agent-guide split become a one-line pointer.

Then it checks the result (identity, name grep, jinn grep, paths, private block, messages, and `just lint`, `just test`, `sh scripts/test-install.sh` on the tree). The same branch and map always give the same commit ids, so later publishes are plain pushes.

```
python3 .agents/publish/publish.py --map-only --ref main          # check the map and that every commit is covered
python3 .agents/publish/publish.py --ref main --out /tmp/orb-pub  # build and check, no push
python3 .agents/publish/publish.py --ref main --email <id>+zqccer@users.noreply.github.com --push <url>
python3 .agents/publish/publish.py --check-only /tmp/orb-pub      # re-run the checks on a built repo
```

- **The email must be the account's GitHub noreply address** (Settings → Emails). `--push` refuses the placeholder.
- **New commits must already be conventional commits.** Add a map row only to reword an old one.
- **Never `--force` a publish.** A rejected push means the ids changed; find out why first.
