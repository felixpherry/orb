# Demo kit

Records the README demo (`docs/demo/demo.gif` and `demo.mp4`) with [VHS](https://github.com/charmbracelet/vhs) against a throwaway home at `/private/tmp/orbdemo/home`. Nothing outside that folder is touched, except that the Incognito scene runs a shell in `/tmp/orb-incognito`.

You need `vhs`, `ttyd`, `ffmpeg`, `zmx`, `python3`, JetBrainsMono Nerd Font and a release build (`cargo build --release`).

```sh
bash docs/demo/kit/rec.sh demo.tape    # reset the home, record, screenshot each scene into shots/
bash docs/demo/kit/sheets.sh           # tile shots/ into 2x2 review sheets
cp docs/demo/kit/demo.{gif,mp4} docs/demo/
```

- `setup.py` builds the home: three git repos, a worktree, a Research folder, `orb integration install`, transcripts, and orb's store seeded with live and settled sessions whose panes come back on start.
- `fakeagent.py` (as `fakebin/claude` and `fakebin/pi`) plays scripted sessions. The fake Claude runs orb's hook and answers `claude agents --json --all`; the fake pi writes its pane file and session file. Replies to new prompts are matched by a word in the prompt (`~/.fakeagent/replies.json`).
- `keys.py` sits between VHS and orb. VHS can't send Cmd or kitty keys, so the tape types markers that `keys.py` rewrites: `⌘x` is Cmd+x, `⌃H`/`⌃L` are Ctrl+Shift+h/l, `⌃[`/`⌃]` are Ctrl+[/], and `🖱x,y;` clicks column x, row y.
- `kill.sh` stops the demo's zmx sessions and fake agents. The zmx sessions live under the demo home, apart from the ones your own orb runs.
