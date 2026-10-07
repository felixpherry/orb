Output demo.mp4
Output demo.gif
Set Shell "bash"
Set FontFamily "JetBrainsMono Nerd Font"
Set FontSize 16
Set Width 1600
Set Height 900
Set Padding 0
Set Margin 0
Set Framerate 30
Set TypingSpeed 70ms
Set Theme { "name": "tokyonight-moon", "black": "#1b1d2b", "red": "#ff757f", "green": "#c3e88d", "yellow": "#ffc777", "blue": "#82aaff", "magenta": "#c099ff", "cyan": "#86e1fc", "white": "#828bb8", "brightBlack": "#444a73", "brightRed": "#ff757f", "brightGreen": "#c3e88d", "brightYellow": "#ffc777", "brightBlue": "#82aaff", "brightMagenta": "#c099ff", "brightCyan": "#86e1fc", "brightWhite": "#c8d3f5", "background": "#222436", "foreground": "#c8d3f5", "selection": "#2d3f76", "cursor": "#c8d3f5" }

# Markers the keys.py wrapper turns into kitty keys: ⌘x is Cmd+x, ⌃H/⌃L are
# Ctrl+Shift+h/l, ⌃[/⌃] are Ctrl+[/], and 🖱x,y; is a click at column x, row y.

Hide
Type "source launch.sh"
Enter
Sleep 1s
Type "orb"
Enter
Sleep 12s
Show

# --- the sidebar: session cards with their agents, and the dashboard ---
Sleep 3s
Screenshot shots/01-start.png
Type "j"
Sleep 600ms
Type "j"
Sleep 600ms
Type "k"
Sleep 600ms
Type "k"
Sleep 600ms

# --- which-key ---
Ctrl+G
Sleep 2s
Screenshot shots/02-whichkey.png
Type "g"
Sleep 1.5s
Screenshot shots/03-whichkey-new.png
Escape
Sleep 600ms

# --- approve a tool call ---
Type "j"
Sleep 800ms
Enter
Sleep 2.5s
Screenshot shots/04-approval.png
Type "1"
Sleep 3s
Screenshot shots/05-working.png
Sleep 8s
Screenshot shots/06-approved.png

# --- panes: focus, add, an agent in a new pane, zoom, tabs ---
Type "⌘h"
Sleep 1.2s
Screenshot shots/07-focus-left.png
Type "⌘l"
Sleep 600ms
Type "⌘j"
Sleep 1s
Type "git log --oneline -3"
Enter
Sleep 1.5s
Screenshot shots/08-shell.png
Type "⌘n"
Sleep 2s
Screenshot shots/09-added-pane.png
Type "claude"
Enter
Sleep 2.5s
Type "review the token refresh fix"
Enter
Sleep 6s
Screenshot shots/10-new-agent.png
Ctrl+G
Sleep 800ms
Type "p"
Sleep 1.5s
Screenshot shots/11-whichkey-pane.png
Type "f"
Sleep 1.5s
Screenshot shots/12-zoom.png
Ctrl+G
Type "pf"
Sleep 1s
Ctrl+G
Type "tn"
Sleep 1.5s
Screenshot shots/13-new-tab.png
Type "🖱93,1;"
Sleep 1.5s
Screenshot shots/14-click-tab.png
Type "⌘1"
Sleep 1.5s

# --- answer a question ---
Type "⌃H"
Sleep 1s
Type "/"
Sleep 400ms
Type "bundle"
Sleep 1s
Screenshot shots/15-search.png
Enter
Sleep 800ms
Enter
Sleep 2s
Screenshot shots/16-question.png
Type "lazy-load the Overview page"
Enter
Sleep 7s
Screenshot shots/17-answered.png
Type "⌃H"
Sleep 1s

# --- fold, rename, pin ---
Type "gg"
Sleep 600ms
Type "h"
Sleep 1s
Screenshot shots/18-folded.png
Type "l"
Sleep 800ms
Type "r"
Sleep 1s
Screenshot shots/19-rename.png
Backspace 40
Type "rate-limiting"
Enter
Sleep 1.5s
Screenshot shots/20-renamed.png

# --- the session picker and transcript search ---
Ctrl+G
Type " "
Sleep 2s
Screenshot shots/21-sessions.png
Type "dark"
Sleep 1.5s
Screenshot shots/22-sessions-dark.png
Escape
Sleep 600ms
Ctrl+G
Type "/"
Sleep 1s
Type "frozen clock"
Sleep 2s
Screenshot shots/23-grep.png
Enter
Sleep 2s
Screenshot shots/24-grep-jump.png

# --- the jump list ---
Type "⌃["
Sleep 1.5s
Screenshot shots/25-jump-back.png
Type "⌃]"
Sleep 1.5s
Type "⌃H"
Sleep 800ms

# --- settle with its confirm, the Settled shelf, resume ---
Type "/"
Sleep 400ms
Type "json"
Sleep 800ms
Enter
Sleep 600ms
Type "h"
Sleep 600ms
Type "s"
Sleep 1.5s
Screenshot shots/26-settle-confirm.png
Down
Sleep 500ms
Enter
Sleep 2s
Screenshot shots/27-settled.png
Type "G"
Sleep 600ms
Enter
Sleep 1.5s
Screenshot shots/28-shelf.png
Type "j"
Sleep 600ms
Type "j"
Sleep 600ms
Enter
Sleep 5s
Screenshot shots/29-resumed.png
Type "⌃H"
Sleep 800ms

# --- a new session in a new worktree, with Claude and pi ---
Ctrl+G
Type "n"
Sleep 1.5s
Screenshot shots/30-projects.png
Type "shipit"
Sleep 600ms
Enter
Sleep 1.5s
Screenshot shots/31-workspace.png
Down
Sleep 500ms
Enter
Sleep 1.5s
Screenshot shots/32-base.png
Enter
Sleep 3s
Screenshot shots/33-new-session.png
Type "claude"
Enter
Sleep 2.5s
Type "add shell completions for zsh and fish"
Enter
Sleep 10s
Screenshot shots/34-completions.png
Type "⌘n"
Sleep 1.5s
Type "pi"
Enter
Sleep 2s
Type "review the completions"
Enter
Sleep 6s
Screenshot shots/35-pi.png
Type "⌃H"
Sleep 1.5s
Screenshot shots/36-sidebar.png

# --- Research and Incognito sessions ---
Ctrl+G
Type "gr"
Sleep 1.2s
Type "auth-providers"
Sleep 600ms
Screenshot shots/37-research-name.png
Enter
Sleep 3s
Screenshot shots/38-research.png
Type "⌃H"
Sleep 600ms
Ctrl+G
Type "i"
Sleep 2.5s
Type "claude"
Enter
Sleep 2.5s
Type "what's a regex for semver?"
Enter
Sleep 6s
Screenshot shots/39-incognito.png
Type "⌃H"
Sleep 800ms

# --- add a project ---
Ctrl+G
Type "a"
Sleep 1.5s
Screenshot shots/40-dirpicker.png
Type "dev"
Sleep 600ms
Tab
Sleep 1.2s
Type "rfcs"
Sleep 800ms
Screenshot shots/41-dirpicker-dev.png
Enter
Sleep 1.5s

# --- the worktree picker ---
Ctrl+G
Type "W"
Sleep 4s
Screenshot shots/42-worktrees.png
Escape
Sleep 600ms

# --- hide and resize the sidebar ---
Ctrl+G
Type "e"
Sleep 1.5s
Screenshot shots/43-hidden.png
Ctrl+G
Type "e"
Sleep 1s
Type "⌃H"
Sleep 600ms
Type "⌘="
Sleep 300ms
Type "⌘="
Sleep 300ms
Type "⌘="
Sleep 1s
Screenshot shots/44-wide.png
Type "⌘-"
Sleep 300ms
Type "⌘-"
Sleep 300ms
Type "⌘-"
Sleep 800ms

# --- the project filter ---
Type "⌃H"
Sleep 600ms
Ctrl+G
Type "f"
Sleep 1.5s
Screenshot shots/45-filter.png
Type "storefront"
Sleep 600ms
Enter
Sleep 1.5s
Screenshot shots/46-filtered.png
Ctrl+G
Type "f"
Sleep 800ms
Type "all"
Sleep 600ms
Enter
Sleep 1.5s

# --- quit, and every pane is still there on the next start ---
Ctrl+G
Type "q"
Sleep 2s
Screenshot shots/47-quit.png
Type "orb"
Enter
Sleep 8s
Screenshot shots/48-back.png
Sleep 3s
