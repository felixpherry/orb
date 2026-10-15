# Keybindings

Rules for adding or changing a key in orb.

- **Bind only keys the maintainer asked for.** A key exists because it is listed in the [README's Keys section](../../README.md#keys) or in an approved plan. Don't add a "handy" extra, and don't take a key nobody asked for.
- **orb's direct keys use Alt.** They are matched in `alt_route` (crates/orb-tui/src/keymap.rs:548). Shift is ignored there, so `Alt +` works however the terminal reports it.
  - On Linux there is nothing to set up.
  - On macOS, Option is often taken by the system or a window manager. The user either sets `macos_option_as_alt yes` in kitty.conf, or adds one kitty map line per key that turns Cmd into Alt, such as `map cmd+h send_key alt+h`. The README's Setup section lists every line.
  - **When you add an Alt key, add its Cmd map line to that README list.**
- **Never bind Super.** orb reads no Super key, and the Mac setup above depends on Cmd reaching orb as Alt.
- **Everything else goes behind the `<C-g>` leader.** The which-key popup labels each key with its intent's `Display` text (crates/orb-domain/src/intent.rs:239), so give a new intent a short label.
- **orb expects kitty.** Some keys only work with the kitty keyboard protocol. `<C-[>` and `<C-]>` need its disambiguate flag, or Ctrl+[ arrives as a plain Esc (`jump_route`, keymap.rs:537).
- **Put each binding in the right `Scope`** (keymap.rs:111-142): the sidebar, a pane, a colony or Incognito session, an agent row. A key bound in the wrong scope fires where it shouldn't, or not at all.
- **Add a keymap test case next to the others** for the new key, the way `ctrl_brackets_jump_in_the_sidebar_and_in_a_pane` covers the jump keys (keymap.rs:1557).
