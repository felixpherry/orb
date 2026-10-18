//! What `orb help` and `--help` print, and the one table of commands that
//! usage lines, help and usage errors all read.
//!
//! Help is plain text with no colour. `orb help` gives an overview, `orb
//! <noun> --help` one line per verb, and `orb <noun> <verb> --help` the usage
//! line, each word the command takes with its default, examples and the
//! reply's shape. A word typed wrong is matched to the closest known one.

use unicode_segmentation::UnicodeSegmentation;

/// One noun of the API, as `orb help` lists it.
#[derive(Debug)]
pub(crate) struct NounHelp {
    pub name: &'static str,
    /// What its commands act on, one line.
    pub summary: &'static str,
}

/// One word a command takes after its verb: a positional (`<pane>`), a
/// flag (`--lines`), or `--`, after which every word is kept as typed.
#[derive(Debug)]
pub(crate) struct Param {
    pub name: &'static str,
    /// What a flag's value looks like (`<n>`, `right|down`); `None` for a
    /// switch, a positional or `--`.
    pub value: Option<&'static str>,
    /// What leaving it out means (`the caller's pane`); `None` when it's
    /// required or a switch.
    pub default: Option<&'static str>,
    pub meaning: &'static str,
    /// May be given more than once (`--link`).
    pub repeats: bool,
}

/// One command: what usage lines, help and usage errors say about it.
#[derive(Debug)]
pub(crate) struct CommandHelp {
    pub noun: &'static str,
    pub verb: &'static str,
    /// The usage line, as usage errors and help print it.
    pub usage: &'static str,
    pub summary: &'static str,
    pub params: &'static [Param],
    /// Whole command lines, double quotes around words with spaces.
    pub examples: &'static [&'static str],
    /// The reply's shape on success.
    pub reply: &'static str,
}

/// The nouns, in the order `orb help` lists them.
pub(crate) const NOUNS: &[NounHelp] = &[
    NounHelp {
        name: "session",
        summary: "orb's sessions: list, make, name, settle and show them",
    },
    NounHelp {
        name: "tab",
        summary: "a session's tabs: list, add and close them",
    },
    NounHelp {
        name: "pane",
        summary: "panes: add and close them, type into them, run commands and read their screens",
    },
    NounHelp {
        name: "agent",
        summary: "Claude and pi agents in panes: start, prompt, wait on and read them",
    },
    NounHelp {
        name: "plate",
        summary: "the user's list of what needs them: add, change, close and list items",
    },
    NounHelp {
        name: "project",
        summary: "orb's projects: list them to find the ID orb session create takes",
    },
];

/// A word that takes no value and has no default: a required positional, a
/// switch (off unless given) or `--`.
const fn word(name: &'static str, meaning: &'static str) -> Param {
    Param {
        name,
        value: None,
        default: None,
        meaning,
        repeats: false,
    }
}

/// A flag that takes a value of the shape `value`, `default` when left out.
const fn flag(
    name: &'static str,
    value: &'static str,
    default: Option<&'static str>,
    meaning: &'static str,
) -> Param {
    Param {
        name,
        value: Some(value),
        default,
        meaning,
        repeats: false,
    }
}

/// A positional that may be left out, meaning `default`.
const fn optional(name: &'static str, default: &'static str, meaning: &'static str) -> Param {
    Param {
        name,
        value: None,
        default: Some(default),
        meaning,
        repeats: false,
    }
}

/// What leaving out a session means.
const CALLERS_SESSION: &str = "the caller's session";

/// What leaving out a pane means.
const CALLERS_PANE: &str = "the caller's pane";

/// `--session <id>`, the caller's session when left out.
const SESSION_FLAG: Param = flag("--session", "<id>", Some(CALLERS_SESSION), "the session");

/// `--name <text>` for a new pane.
const NAME_FLAG: Param = flag(
    "--name",
    "<text>",
    Some("the usual name"),
    "a name for the new pane",
);

/// `--run <command>` for a new pane.
const RUN_FLAG: Param = flag(
    "--run",
    "<command>",
    Some("nothing"),
    "a command line typed into the new shell, then Enter, once the shell first prints",
);

/// `--cwd <dir>` for a new pane.
const CWD_FLAG: Param = flag(
    "--cwd",
    "<dir>",
    Some("the session's folder"),
    "the folder the new shell starts in; a relative one is from where you run orb",
);

/// `[<pane>]`, the caller's pane when left out.
const OPTIONAL_PANE: Param = optional("<pane>", CALLERS_PANE, "the pane's ID");

/// `<pane>`, a pane's ID that must be given.
const PANE: Param = word("<pane>", "the pane's ID");

/// `<pane>`, the pane an agent runs in.
const AGENT_PANE: Param = word("<pane>", "the pane the agent runs in");

/// `<item>`, a plate item's ID.
const ITEM: Param = word("<item>", "the item's ID, as orb plate list shows it");

/// `--link <url>`, which may repeat.
const LINK: Param = Param {
    name: "--link",
    value: Some("<url>"),
    default: None,
    meaning: "a web address (https://…) or a file path, which opens in $EDITOR",
    repeats: true,
};

/// The plate's verbs, as `--verb` takes them.
const PLATE_VERBS: &str = "the user's next step: decide (pick a direction), review (a diff or PR), \
     read (a report or doc), walk (try it by hand), pick (choose among options), \
     approve (a plan or a step), answer (a question you asked), reply (someone else's message), \
     merge (a branch that's ready), note (an FYI) or idea (a follow-up you offer; it lands parked)";

/// Every agent status, as `--status` takes them.
const STATUSES: &str = "unknown|working|needs_approval|needs_input|idle|failed|stopped|gone";

/// What a change replies once done.
const DONE: &str = "{}";

/// What an add replies: the new pane.
const NEW_PANE: &str = "{\"pane\": N}";

/// What a pane or tab close replies: each pane it ended, with the pane's own
/// name and the live agent's name, each null when there is none.
const ENDED: &str = "{\"ended\": [{\"pane\": N, \"name\": \"…\"|null, \"agent\": \"…\"|null}]}";

/// Every command, in the order the usage lines list them.
pub(crate) const COMMANDS: &[CommandHelp] = &[
    CommandHelp {
        noun: "session",
        verb: "list",
        usage: "orb session list [--all] [--project <id>]",
        summary: "List open sessions; --all adds settled ones.",
        params: &[
            word("--all", "settled sessions too"),
            flag(
                "--project",
                "<id>",
                Some("every project"),
                "only this project's sessions; orb project list shows the IDs",
            ),
        ],
        examples: &[
            "orb session list",
            "orb session list --all",
            "orb session list --project 4",
        ],
        reply: "{\"sessions\": [{\"id\", \"project\", \"name\", \"dir\", \"branch\", \"settled\"}], \"hidden\": N, \"hint\"}; \
                hidden counts the settled sessions left out, hint (only when hidden > 0) says how to list them; \
                names are cut to 60 characters",
    },
    CommandHelp {
        noun: "tab",
        verb: "list",
        usage: "orb tab list [--session <id>]",
        summary: "List a session's tabs and the panes in each.",
        params: &[SESSION_FLAG],
        examples: &["orb tab list", "orb tab list --session 7"],
        reply: "{\"session\": N, \"tabs\": [{\"index\": N, \"panes\": [N]}]}; tabs count from 1",
    },
    CommandHelp {
        noun: "pane",
        verb: "list",
        usage: "orb pane list [--session <id>]",
        summary: "List a session's panes, with the caller's marked.",
        params: &[SESSION_FLAG],
        examples: &["orb pane list", "orb pane list --session 7"],
        reply: "{\"panes\": [{\"id\", \"session\", \"tab\", \"float\", \"name\", \"cwd\", \"agent\", \"caller\"}]}",
    },
    CommandHelp {
        noun: "agent",
        verb: "list",
        usage: "orb agent list [--all] [--session <id>] [--status <status>[,<status>...]]",
        summary: "List live agents; --all adds stopped and gone ones.",
        params: &[
            word("--all", "stopped and gone agents too"),
            flag(
                "--session",
                "<id>",
                Some("every session"),
                "only this session's agents",
            ),
            flag(
                "--status",
                STATUSES,
                Some("every status but stopped and gone"),
                "only agents with these statuses; separate several with commas",
            ),
        ],
        examples: &[
            "orb agent list",
            "orb agent list --status needs_approval,needs_input",
            "orb agent list --session 7 --all",
        ],
        reply: "{\"agents\": [{\"id\", \"pane\", \"session\", \"harness\", \"name\", \"title\", \"status\"}], \"hidden\": N, \"hint\"}; \
                hidden counts the stopped and gone agents left out, hint (only when hidden > 0) says how to list them; \
                title only when it differs from name, both cut to 60 characters; \
                orb agent get <pane> has the full row with cwd and branch",
    },
    CommandHelp {
        noun: "project",
        verb: "list",
        usage: "orb project list",
        summary: "List orb's projects (removed ones left out), to find a project's ID.",
        params: &[],
        examples: &["orb project list"],
        reply: "{\"projects\": [{\"id\", \"name\", \"dir\", \"kind\"}]}; \
                kind is repo, folder, research, learn, incognito, shared_world or fresh_world",
    },
    CommandHelp {
        noun: "session",
        verb: "create",
        usage: "orb session create [--project <id>] [--worktree] [--name <text>]",
        summary: "Make a new session; it shows in the sidebar without opening. \
                  From a colony's pane it becomes a colonist.",
        params: &[
            flag(
                "--project",
                "<id>",
                Some("the caller's session's project"),
                "the project's ID, as orb project list shows it",
            ),
            word(
                "--worktree",
                "work in a new git worktree from the default branch, not the project's checkout",
            ),
            flag(
                "--name",
                "<text>",
                Some("the usual name"),
                "the session's name",
            ),
        ],
        examples: &[
            "orb session create --name \"fix login\"",
            "orb session create --project 4 --worktree --name \"fix payout\"",
        ],
        reply: "{\"session\": N, \"pane\": N}; the pane is a shell",
    },
    CommandHelp {
        noun: "tab",
        verb: "create",
        usage: "orb tab create [--session <id>] [--name <text>] [--run <command>] [--cwd <dir>]",
        summary: "Add a tab with one shell at the end of a session's tabs.",
        params: &[SESSION_FLAG, NAME_FLAG, RUN_FLAG, CWD_FLAG],
        examples: &[
            "orb tab create",
            "orb tab create --name server --run \"just dev\"",
            "orb tab create --session 7 --cwd ../web",
        ],
        reply: NEW_PANE,
    },
    CommandHelp {
        noun: "pane",
        verb: "split",
        usage: "orb pane split [<pane>] --direction right|down [--name <text>] [--run <command>] [--cwd <dir>]",
        summary: "Add a shell beside a pane (or its whole stack).",
        params: &[
            OPTIONAL_PANE,
            flag(
                "--direction",
                "right|down",
                None,
                "which side the new pane goes",
            ),
            NAME_FLAG,
            RUN_FLAG,
            CWD_FLAG,
        ],
        examples: &[
            "orb pane split --direction right",
            "orb pane split --direction right --name tests --run \"just test\"",
            "orb pane split 12 --direction down --cwd /tmp",
        ],
        reply: NEW_PANE,
    },
    CommandHelp {
        noun: "pane",
        verb: "stack",
        usage: "orb pane stack [<pane>] [--name <text>] [--run <command>] [--cwd <dir>]",
        summary: "Add a shell stacked right after a pane.",
        params: &[OPTIONAL_PANE, NAME_FLAG, RUN_FLAG, CWD_FLAG],
        examples: &[
            "orb pane stack",
            "orb pane stack 12 --name logs --run \"tail -f app.log\"",
        ],
        reply: NEW_PANE,
    },
    CommandHelp {
        noun: "pane",
        verb: "float",
        usage: "orb pane float [--session <id>] [--name <text>] [--run <command>] [--cwd <dir>]",
        summary: "Add a floating shell to a session.",
        params: &[SESSION_FLAG, NAME_FLAG, RUN_FLAG, CWD_FLAG],
        examples: &[
            "orb pane float",
            "orb pane float --session 7 --name scratch",
            "orb pane float --run htop",
        ],
        reply: NEW_PANE,
    },
    CommandHelp {
        noun: "pane",
        verb: "close",
        usage: "orb pane close <pane>",
        summary: "Close a pane and whatever runs in it.",
        params: &[PANE],
        examples: &["orb pane close 12"],
        reply: ENDED,
    },
    CommandHelp {
        noun: "pane",
        verb: "rename",
        usage: "orb pane rename <pane> <name>",
        summary: "Name a pane; an empty name goes back to the usual one.",
        params: &[PANE, word("<name>", "the new name, one quoted word")],
        examples: &[
            "orb pane rename 12 server",
            "orb pane rename 12 \"dev server\"",
        ],
        reply: DONE,
    },
    CommandHelp {
        noun: "pane",
        verb: "focus",
        usage: "orb pane focus <pane>",
        summary: "Show a pane and move the user's keys into it.",
        params: &[PANE],
        examples: &["orb pane focus 12"],
        reply: DONE,
    },
    CommandHelp {
        noun: "tab",
        verb: "close",
        usage: "orb tab close [--pane <id>]",
        summary: "Close the tab holding a pane, with every pane in it.",
        params: &[flag(
            "--pane",
            "<id>",
            Some(CALLERS_PANE),
            "a pane in the tab",
        )],
        examples: &["orb tab close --pane 12"],
        reply: ENDED,
    },
    CommandHelp {
        noun: "session",
        verb: "rename",
        usage: "orb session rename [<session>] <name>",
        summary: "Name a session; an empty name goes back to the usual one.",
        params: &[
            optional("<session>", CALLERS_SESSION, "the session's ID"),
            word("<name>", "the new name, one quoted word"),
        ],
        examples: &[
            "orb session rename \"fix login\"",
            "orb session rename 7 \"fix login\"",
        ],
        reply: DONE,
    },
    CommandHelp {
        noun: "session",
        verb: "settle",
        usage: "orb session settle [<session>]",
        summary: "Move a session to the Settled shelf, closing its panes.",
        params: &[optional("<session>", CALLERS_SESSION, "the session's ID")],
        examples: &["orb session settle 7"],
        reply: "{\"session\": N, \"name\": \"…\", \"ended\": [{\"pane\": N, \"name\": \"…\"|null, \"agent\": \"…\"|null}]}",
    },
    CommandHelp {
        noun: "session",
        verb: "focus",
        usage: "orb session focus <session>",
        summary: "Show a session's focused pane and move the user's keys into it.",
        params: &[word("<session>", "the session's ID")],
        examples: &["orb session focus 7"],
        reply: DONE,
    },
    CommandHelp {
        noun: "pane",
        verb: "send",
        usage: "orb pane send <pane> <text>",
        summary: "Paste text into a pane, without Enter.",
        params: &[
            PANE,
            word("<text>", "the text, one quoted word, taken as is"),
        ],
        examples: &["orb pane send 12 \"hello there\""],
        reply: DONE,
    },
    CommandHelp {
        noun: "pane",
        verb: "keys",
        usage: "orb pane keys <pane> <key>...",
        summary: "Press named keys in a pane, in order; an unknown name sends nothing.",
        params: &[
            PANE,
            Param {
                name: "<key>",
                value: None,
                default: None,
                meaning: "enter esc tab backspace space delete insert up down left right home \
                          end pageup pagedown f1-f12 shift+tab ctrl+<a-z> in any case, \
                          alt+<char>; or one character, sent as its key (uppercase as shifted)",
                repeats: true,
            },
        ],
        examples: &[
            "orb pane keys 12 ctrl+c",
            "orb pane keys 12 down down enter",
            "orb pane keys 12 esc",
            "orb pane keys 12 q",
        ],
        reply: DONE,
    },
    CommandHelp {
        noun: "pane",
        verb: "type",
        usage: "orb pane type <pane> <text>",
        summary: "Type text into a pane key by key: no paste, no Enter; control characters \
                  are left out.",
        params: &[
            PANE,
            word("<text>", "the text, one quoted word, taken as is"),
        ],
        examples: &["orb pane type 12 \":wq\"", "orb pane type 12 \"hello\""],
        reply: DONE,
    },
    CommandHelp {
        noun: "pane",
        verb: "run",
        usage: "orb pane run <pane> <command> [--wait] [--timeout <ms>] [--force] [--tail <n>] [--json]",
        summary: "Type a command line and Enter into a pane's shell; a pane not drawn yet gets it \
                  once it is. In a shell that sends prompt marks (fish does), it's refused while \
                  a program runs there unless --force, and text left on the command line is \
                  cleared first. With --wait, print the command's output and exit code once it \
                  ends.",
        params: &[
            PANE,
            word(
                "<command>",
                "the command line, one quoted word; put -- before it if it starts with --",
            ),
            word("--wait", "wait for the command to end and print its output"),
            flag(
                "--timeout",
                "<ms>",
                Some("no limit; only with --wait"),
                "milliseconds the whole call may take",
            ),
            word("--force", "type even while a program runs in the pane"),
            flag(
                "--tail",
                "<n>",
                Some("200; only with --wait"),
                "keep the output's last n lines",
            ),
            word(
                "--json",
                "print {\"exit\", \"output\", \"lines\", \"truncated\"} as JSON instead of plain text",
            ),
            word(
                "--",
                "the command line follows, even when it starts with --",
            ),
        ],
        examples: &[
            "orb pane run 12 \"just test\"",
            "orb pane run 12 \"cargo build\" --wait --timeout 600000",
            "orb pane run 12 \"git status\" --wait --json",
        ],
        reply: "{} ; with --wait the output's last lines as plain text, then [exit n] ([exit ?] \
                in a shell that sends no prompt marks, where it returns once the screen is quiet \
                for 1 s, no sooner than 2 s after typing); with --json {\"exit\": n|null, \"output\": \"…\", \"lines\": N, \
                \"truncated\": bool}",
    },
    CommandHelp {
        noun: "pane",
        verb: "read",
        usage: "orb pane read [<pane>] [--lines <n>] [--head <n>] [--tail <n>] [--json]",
        summary: "Print a pane's screen as plain text, with lines of history above it; trailing spaces and extra blank lines are trimmed.",
        params: &[
            OPTIONAL_PANE,
            flag(
                "--lines",
                "<n>",
                Some("0, the screen only"),
                "how many lines of history to add above the screen",
            ),
            flag(
                "--head",
                "<n>",
                Some("every line"),
                "keep the first n lines, after trimming",
            ),
            flag(
                "--tail",
                "<n>",
                Some("every line"),
                "keep the last n lines, after trimming and --head",
            ),
            word(
                "--json",
                "print {\"text\": …} as JSON instead of plain text",
            ),
        ],
        examples: &[
            "orb pane read 12",
            "orb pane read 12 --tail 20",
            "orb pane read 12 --lines 200 --tail 50",
            "orb pane read 12 --json",
        ],
        reply: "the trimmed screen as plain text; with --json {\"text\": \"…\"}",
    },
    CommandHelp {
        noun: "pane",
        verb: "wait",
        usage: "orb pane wait <pane> (--text <text> | --regex <pattern>) [--timeout <ms>] [--lines <n>] [--json]",
        summary: "Wait until a line of a pane's screen or recent history holds the text or matches \
                  the pattern, and print that line. Text already on screen counts, so wait for \
                  text only the new output prints.",
        params: &[
            PANE,
            flag("--text", "<text>", None, "text a line must hold"),
            flag(
                "--regex",
                "<pattern>",
                None,
                "a pattern a line must match, instead of --text",
            ),
            flag(
                "--timeout",
                "<ms>",
                Some("until you hang up"),
                "milliseconds to wait before giving up",
            ),
            flag(
                "--lines",
                "<n>",
                Some("200"),
                "how many lines of history above the screen to search",
            ),
            word(
                "--json",
                "print {\"line\": …} as JSON instead of plain text",
            ),
        ],
        examples: &[
            "orb pane wait 12 --text \"Listening on\" --timeout 60000",
            "orb pane wait 12 --regex \"test result: (ok|FAILED)\"",
        ],
        reply: "the first matching line as plain text; with --json {\"line\": \"…\"}",
    },
    CommandHelp {
        noun: "agent",
        verb: "start",
        usage: "orb agent start --kind claude|pi [--pane <id> | --split right|down | --tab] [--name <text>] [--force] [-- <args>...]",
        summary: "Type claude or pi into a shell pane, or a new one, and wait until orb sees it \
                  running (up to 30 s). In a shell that sends prompt marks (fish does), it's \
                  refused while a program runs in the pane unless --force.",
        params: &[
            flag("--kind", "claude|pi", None, "which agent to start"),
            flag(
                "--pane",
                "<id>",
                Some(CALLERS_PANE),
                "a shell pane; without --pane, --split or --tab, your own",
            ),
            flag(
                "--split",
                "right|down",
                None,
                "start in a new pane split off yours",
            ),
            word("--tab", "start in a new tab of your session"),
            flag(
                "--name",
                "<text>",
                Some("the usual name"),
                "a name for the pane",
            ),
            word(
                "--force",
                "type the program even while another runs in the pane; not with --split or --tab",
            ),
            word(
                "--",
                "every word after it is typed after the program, each quoted",
            ),
        ],
        examples: &[
            "orb agent start --kind claude --split right --name helper",
            "orb agent start --kind pi --tab -- --model sonnet",
            "orb agent start --kind claude --pane 12 --name reviewer",
        ],
        reply: "{\"pane\": N, \"agent\": N, \"harness\": \"claude\"|\"pi\", \"status\": \"idle\"|\"working\"}",
    },
    CommandHelp {
        noun: "agent",
        verb: "prompt",
        usage: "orb agent prompt <pane> <text> [--wait [--timeout <ms>]] [--json]",
        summary: "Paste a prompt into an agent and press Enter.",
        params: &[
            AGENT_PANE,
            word(
                "<text>",
                "the prompt, one quoted word; after -- when it starts with -",
            ),
            word(
                "--wait",
                "wait for the turn to end, then print what the agent wrote since this prompt",
            ),
            flag(
                "--timeout",
                "<ms>",
                Some("no limit while work goes on"),
                "milliseconds the whole call may take; only with --wait",
            ),
            word(
                "--json",
                "print {\"status\", \"text\", \"at\"} as JSON instead of plain text",
            ),
            word("--", "the prompt follows, even when it starts with -"),
        ],
        examples: &[
            "orb agent prompt 12 \"Review the diff on this branch.\" --wait",
            "orb agent prompt 12 \"Fix the login bug.\"",
            "orb agent prompt 12 \"Summarise the plan.\" --wait --json",
            "orb agent prompt 12 --wait --timeout 600000 -- \"--force is broken\"",
        ],
        reply: "{\"submitted\": true}; with --wait the answer as plain text, or with --json {\"status\": \"idle\", \"text\": \"…\", \"at\": unix milliseconds|null}; agent_blocked when the agent stops to ask the user, turn_failed when its model call fails; no_reply when it wrote nothing within 2 s of the turn ending",
    },
    CommandHelp {
        noun: "agent",
        verb: "wait",
        usage: "orb agent wait <pane> [--timeout <ms>]",
        summary: "Wait until a pane's agent is idle or stops to ask the user.",
        params: &[
            AGENT_PANE,
            flag(
                "--timeout",
                "<ms>",
                Some("no limit while work goes on"),
                "milliseconds to wait",
            ),
        ],
        examples: &["orb agent wait 12", "orb agent wait 12 --timeout 600000"],
        reply: "{\"status\": \"idle\"|\"blocked\"}; a prompt orb sent under 5 s ago counts as work; turn_failed when the agent's model call failed",
    },
    CommandHelp {
        noun: "agent",
        verb: "get",
        usage: "orb agent get <pane>",
        summary: "Show a pane's agent in full: whole name and title, cwd and branch.",
        params: &[AGENT_PANE],
        examples: &["orb agent get 12"],
        reply: "{\"id\", \"pane\", \"session\", \"harness\", \"name\", \"title\", \"status\", \"cwd\", \"branch\"}; status reads \"working\" for 5 s after orb's prompt",
    },
    CommandHelp {
        noun: "agent",
        verb: "reply",
        usage: "orb agent reply <pane> [--json]",
        summary: "Print everything a pane's agent wrote since orb's last prompt to it, else its last reply, as plain text.",
        params: &[
            AGENT_PANE,
            word(
                "--json",
                "print {\"text\", \"at\"} as JSON instead of plain text",
            ),
        ],
        examples: &["orb agent reply 12", "orb agent reply 12 --json"],
        reply: "the reply as plain text; with --json {\"text\": \"…\", \"at\": unix milliseconds|null}; turn_failed when the turn ended in a failed model call",
    },
    CommandHelp {
        noun: "plate",
        verb: "add",
        usage: "orb plate add <title> --verb <verb> [--what <text>] [--link <url>]... [--urgent] [--waiting] [--pane <id>]",
        summary: "Put an item on the user's plate when a turn ends needing them.",
        params: &[
            word("<title>", "the user's next step, as an action"),
            flag("--verb", "<verb>", None, PLATE_VERBS),
            flag(
                "--what",
                "<text>",
                Some("nothing"),
                "what happened, a sentence or two",
            ),
            LINK,
            word("--urgent", "it blocks other work; urgent items sort first"),
            word("--waiting", "the next move is someone else's"),
            flag(
                "--pane",
                "<id>",
                Some(CALLERS_PANE),
                "where the user's ⏎ goes",
            ),
        ],
        examples: &[
            "orb plate add \"Review PR #42: fix login\" --verb review --link https://github.com/o/r/pull/42",
            "orb plate add \"Add a retry to payouts?\" --verb idea",
        ],
        reply: "{\"item\": N}",
    },
    CommandHelp {
        noun: "plate",
        verb: "update",
        usage: "orb plate update <item> [--title <text>] [--verb <verb>] [--what <text>] [--link <url>]... [--urgent|--not-urgent] [--waiting|--your-move]",
        summary: "Change an open item; what's left out stays.",
        params: &[
            ITEM,
            flag(
                "--title",
                "<text>",
                Some("unchanged"),
                "the user's next step",
            ),
            flag("--verb", "<verb>", Some("unchanged"), PLATE_VERBS),
            flag(
                "--what",
                "<text>",
                Some("unchanged"),
                "what happened, a sentence or two",
            ),
            Param {
                name: "--link",
                value: Some("<url>"),
                default: Some("unchanged"),
                meaning: "replaces every link; a web address or a file path",
                repeats: true,
            },
            word("--urgent", "it blocks other work"),
            word("--not-urgent", "it no longer does"),
            word("--waiting", "the next move is someone else's"),
            word("--your-move", "the next move is the user's again"),
        ],
        examples: &[
            "orb plate update 7 --waiting",
            "orb plate update 7 --title \"Merge PR #42\" --verb merge",
        ],
        reply: DONE,
    },
    CommandHelp {
        noun: "plate",
        verb: "done",
        usage: "orb plate done <item>",
        summary: "Close an item once the user's step is done.",
        params: &[ITEM],
        examples: &["orb plate done 7"],
        reply: DONE,
    },
    CommandHelp {
        noun: "plate",
        verb: "list",
        usage: "orb plate list",
        summary: "List open items and those done today.",
        params: &[],
        examples: &["orb plate list"],
        reply: "{\"items\": [{\"id\", \"verb\", \"state\", \"title\", \"what\", \"links\", \"urgent\", \"session\", \"pane\", \"age_s\"}]}",
    },
];

/// The noun `word` names, as the table spells it.
pub(crate) fn noun(word: &str) -> Option<&'static str> {
    NOUNS
        .iter()
        .find(|noun| noun.name == word)
        .map(|noun| noun.name)
}

/// The nouns' names, in table order.
pub(crate) fn nouns() -> Vec<&'static str> {
    NOUNS.iter().map(|noun| noun.name).collect()
}

/// `noun`'s verbs, in table order.
pub(crate) fn verbs(noun: &str) -> Vec<&'static str> {
    COMMANDS
        .iter()
        .filter(|command| command.noun == noun)
        .map(|command| command.verb)
        .collect()
}

/// The command `noun verb` names.
pub(crate) fn command(noun: &str, verb: &str) -> Option<&'static CommandHelp> {
    COMMANDS
        .iter()
        .find(|command| command.noun == noun && command.verb == verb)
}

/// The common jobs `orb help` shows: what each is for, then its commands.
const RECIPES: &[(&str, &[&str])] = &[
    ("See your session's panes:", &["orb pane list"]),
    (
        "Run a command in a new pane:",
        &["orb pane split --direction right --name <name> --run \"<command>\""],
    ),
    (
        "Ask another agent and read its answer:",
        &[
            "orb agent start --kind claude --split right",
            "orb agent prompt <pane> \"<text>\" --wait",
            "orb agent reply <pane>",
        ],
    ),
    (
        "Run a command in a shell pane and get its output and exit code:",
        &["orb pane run <pane> \"<command>\" --wait"],
    ),
];

/// `orb help`: what the API is for, one line per noun, 4 common jobs,
/// exit codes and reply format, and `orb <noun> --help`.
pub(crate) fn overview() -> String {
    let width = NOUNS.iter().map(|noun| noun.name.len()).max().unwrap_or(0);
    let mut lines = vec![
        "orb: drive the running orb from one of its panes. List and add sessions, tabs and \
         panes, type into panes and read them, start and prompt agents, and put items on the \
         user's plate."
            .to_owned(),
        String::new(),
        "usage: orb <noun> <verb> [<words>...]".to_owned(),
        String::new(),
        "nouns:".to_owned(),
    ];
    lines.extend(
        NOUNS
            .iter()
            .map(|noun| format!("  {:<width$}  {}", noun.name, noun.summary)),
    );
    lines.push(String::new());
    lines.push("common jobs:".to_owned());
    for (job, commands) in RECIPES {
        lines.push(format!("  {job}"));
        lines.extend(commands.iter().map(|command| format!("    {command}")));
    }
    lines.extend([
        String::new(),
        "Bare orb in an orb pane says which pane, tab and session you're in.".to_owned(),
        String::new(),
        "replies:".to_owned(),
        "  exit 0  done; the reply is JSON on stdout, except pane read, pane run --wait, pane wait, agent prompt --wait and agent reply, which print plain text (--json gives JSON)".to_owned(),
        "  exit 1  refused, or orb isn't running; {\"code\", \"message\", \"hint\"} on stderr"
            .to_owned(),
        "  exit 2  typed wrong; what was wrong, the usage line and the help command on stderr"
            .to_owned(),
        String::new(),
        "help: orb <noun> --help · orb <noun> <verb> --help".to_owned(),
    ]);
    lines.join("\n")
}

/// `orb <noun> --help`: one line per verb (verb + summary), then
/// `orb <noun> <verb> --help`.
pub(crate) fn noun_help(noun: &str) -> String {
    let summary = NOUNS
        .iter()
        .find(|known| known.name == noun)
        .map_or("", |known| known.summary);
    let commands: Vec<&CommandHelp> = COMMANDS
        .iter()
        .filter(|command| command.noun == noun)
        .collect();
    let width = commands
        .iter()
        .map(|command| command.verb.len())
        .max()
        .unwrap_or(0);
    let mut lines = vec![
        format!("orb {noun}: {summary}"),
        String::new(),
        "verbs:".to_owned(),
    ];
    lines.extend(
        commands
            .iter()
            .map(|command| format!("  {:<width$}  {}", command.verb, command.summary)),
    );
    lines.push(String::new());
    lines.push(format!("help: orb {noun} <verb> --help"));
    lines.join("\n")
}

/// How a param reads in help: its name, then its value's shape.
fn param_label(param: &Param) -> String {
    match param.value {
        Some(value) => format!("{} {value}", param.name),
        None => param.name.to_owned(),
    }
}

/// What a param means in help, with its default and whether it repeats.
fn param_meaning(param: &Param) -> String {
    let default = param
        .default
        .map(|default| format!(" (default: {default})"))
        .unwrap_or_default();
    let repeats = if param.repeats {
        " (may be given more than once)"
    } else {
        ""
    };
    format!("{}{default}{repeats}", param.meaning)
}

/// `orb <noun> <verb> --help`: summary, usage line, each param with its
/// default, the examples, the reply shape.
pub(crate) fn command_help(command: &CommandHelp) -> String {
    let mut lines = vec![
        format!("orb {} {}: {}", command.noun, command.verb, command.summary),
        String::new(),
        format!("usage: {}", command.usage),
    ];
    if !command.params.is_empty() {
        let labels: Vec<String> = command.params.iter().map(param_label).collect();
        let width = labels.iter().map(String::len).max().unwrap_or(0);
        lines.push(String::new());
        lines.extend(
            labels
                .iter()
                .zip(command.params)
                .map(|(label, param)| format!("  {label:<width$}  {}", param_meaning(param))),
        );
    }
    lines.push(String::new());
    lines.push("examples:".to_owned());
    lines.extend(
        command
            .examples
            .iter()
            .map(|example| format!("  {example}")),
    );
    lines.push(String::new());
    lines.push(format!("reply: {}", command.reply));
    lines.join("\n")
}

/// The most edits a typed word may be from a known one to be suggested.
const MAX_EDITS: usize = 2;

/// The known word closest to `word`: one `word` is a prefix of, else the
/// nearest within 2 edits (counted by grapheme); `None` past that.
pub(crate) fn suggest<'a, I>(word: &str, known: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let known: Vec<&'a str> = known.into_iter().collect();
    if !word.trim_start_matches('-').is_empty()
        && let Some(&longer) = known.iter().find(|known| known.starts_with(word))
    {
        return Some(longer);
    }
    let typed: Vec<&str> = word.graphemes(true).collect();
    known
        .iter()
        .map(|&candidate| {
            let letters: Vec<&str> = candidate.graphemes(true).collect();
            (candidate, edits(&typed, &letters))
        })
        .filter(|&(_, edits)| edits <= MAX_EDITS)
        .min_by_key(|&(_, edits)| edits)
        .map(|(candidate, _)| candidate)
}

/// How many one-grapheme inserts, deletes or swaps turn `from` into `to`.
fn edits(from: &[&str], to: &[&str]) -> usize {
    let mut row: Vec<usize> = (0..=to.len()).collect();
    for (done, letter) in from.iter().enumerate() {
        let mut next = Vec::with_capacity(row.len());
        let mut left = done + 1;
        next.push(left);
        for ((other, &diagonal), &up) in to.iter().zip(&row).zip(row.iter().skip(1)) {
            let cost = usize::from(letter != other);
            left = (diagonal + cost).min(up + 1).min(left + 1);
            next.push(left);
        }
        row = next;
    }
    row.last().copied().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::ffi::OsString;

    use super::{COMMANDS, STATUSES, command, suggest, verbs};
    use crate::feat::control::client::parse;
    use crate::feat::control::protocol::Op;
    use crate::feat::plate::Verb;
    use crate::feat::sessions::state::AgentStatus;

    /// `line` as the shell splits it: on spaces, with double quotes keeping
    /// a word with spaces whole.
    fn shell_words(line: &str) -> Vec<OsString> {
        line.split('"')
            .enumerate()
            .flat_map(|(index, part)| {
                if index % 2 == 1 {
                    vec![part]
                } else {
                    part.split_whitespace().collect()
                }
            })
            .map(OsString::from)
            .collect()
    }

    #[rstest::rstest]
    fn every_example_parses_to_its_own_op() {
        // Given every example of every command.
        let examples = COMMANDS.iter().flat_map(|command| {
            command
                .examples
                .iter()
                .map(move |example| (command, *example))
        });

        // When parsing each one's words after `orb`.
        let wrong: Vec<String> = examples
            .filter_map(|(command, example)| {
                let words = shell_words(example.strip_prefix("orb ").unwrap_or(example));
                let tag = parse(&words).ok().and_then(|op| {
                    serde_json::to_value(op)
                        .ok()?
                        .get("op")?
                        .as_str()
                        .map(str::to_owned)
                });
                let own = format!("{}.{}", command.noun, command.verb);
                (tag.as_deref() != Some(own.as_str())).then(|| format!("{example} -> {tag:?}"))
            })
            .collect();

        // Then each makes the request of the command it illustrates.
        assert_eq!(
            wrong,
            Vec::<String>::new(),
            "every example should parse to its own command's request"
        );
    }

    #[rstest::rstest]
    fn plate_add_help_lists_every_verb() {
        // Given `plate add`'s --verb.
        let meaning = command("plate", "add")
            .and_then(|add| add.params.iter().find(|param| param.name == "--verb"))
            .map_or("", |param| param.meaning);

        // When looking for each plate verb in what it means.
        let missing: Vec<&str> = Verb::ALL
            .iter()
            .map(|verb| verb.name())
            .filter(|name| !meaning.contains(name))
            .collect();

        // Then it names them all.
        assert_eq!(
            missing,
            Vec::<&str>::new(),
            "--verb's help should name every plate verb"
        );
    }

    #[rstest::rstest]
    #[case::prefix("--dir", &["--direction", "--pane"], Some("--direction"))]
    #[case::two_edits("ls", &["list", "split", "stack"], Some("list"))]
    #[case::one_edit("sessions", &["session", "tab", "pane"], Some("session"))]
    #[case::far("zzzz", &["list", "create", "rename"], None)]
    fn suggest_picks_the_closest_known_word(
        #[case] word: &str,
        #[case] known: &[&'static str],
        #[case] expected: Option<&str>,
    ) {
        // Given / When suggesting a known word for `word`.
        let suggested = suggest(word, known.iter().copied());

        // Then it is the closest one, or none when all are too far.
        assert_eq!(suggested, expected, "{word} among {known:?}");
    }

    #[rstest::rstest]
    fn verbs_of_a_noun_come_in_table_order() {
        // Given / When listing the session verbs.
        let listed = verbs("session");

        // Then they follow the table.
        assert_eq!(
            listed,
            vec!["list", "create", "rename", "settle", "focus"],
            "the session verbs in table order"
        );
    }

    #[rstest::rstest]
    fn every_command_in_the_orb_skill_parses() {
        // Given every `orb` line in the orb skill's code blocks, without its
        // `#` comment, leaving out help, which never goes through `parse`.
        let skill = include_str!("../integration/assets/skills/orb/SKILL.md");
        let mut fenced = false;
        let lines: Vec<&str> = skill
            .lines()
            .filter(|line| {
                if line.starts_with("```") {
                    fenced = !fenced;
                    return false;
                }
                fenced
            })
            .filter(|line| line.starts_with("orb "))
            .map(|line| {
                line.split_once("  #")
                    .map_or(line, |(command, _)| command)
                    .trim_end()
            })
            .filter(|line| !line.starts_with("orb help") && !line.contains(" --help"))
            .collect();

        // When parsing each one's words after `orb`.
        let wrong: Vec<String> = lines
            .iter()
            .filter_map(|line| {
                let words = shell_words(line.strip_prefix("orb ").unwrap_or(line));
                parse(&words)
                    .err()
                    .map(|error| format!("{line} -> {error:?}"))
            })
            .collect();

        // Then there are commands to check, and none fails to parse.
        assert!(
            lines.len() >= 20,
            "the orb skill's code blocks should show at least 20 commands, got {}",
            lines.len()
        );
        assert_eq!(
            wrong,
            Vec::<String>::new(),
            "every orb command the skill shows should parse"
        );
    }

    #[rstest::rstest]
    fn status_flag_names_every_status() {
        // Given every status `agent list --status` names in its help.
        let names = STATUSES.replace('|', ",");

        // When parsing them all as one --status list.
        let op = parse(&shell_words(&format!("agent list --status {names}")));

        // Then all 8 are statuses, none of them twice.
        let statuses: HashSet<AgentStatus> = match op {
            Ok(Op::AgentList { status, .. }) => status.into_iter().collect(),
            _ => HashSet::new(),
        };
        assert_eq!(
            statuses.len(),
            8,
            "--status's help names the 8 agent statuses, got {statuses:?}"
        );
    }
}
