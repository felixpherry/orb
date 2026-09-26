//! A thread's conversation, rebuilt from its Claude transcript.
//!
//! Lines are decoded leniently: unknown line types are ignored, lines that
//! can't be read are counted and skipped, and a line seen twice keeps its
//! first copy. The conversation shown is the newest branch — the path back
//! from the newest line along parent links — plus the tool calls and results
//! that belong to it, continuing across compaction.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use super::block::{Block, BlockId, BlockKind, SystemLevel, ToolCall, ToolStatus};

/// One transcript line, with only the fields the preview reads.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLine {
    #[serde(rename = "type")]
    kind: LineKind,
    uuid: Option<String>,
    parent_uuid: Option<String>,
    logical_parent_uuid: Option<String>,
    is_sidechain: Option<bool>,
    is_meta: Option<bool>,
    is_compact_summary: Option<bool>,
    is_api_error_message: Option<bool>,
    git_branch: Option<String>,
    message: Option<RawMessage>,
    tool_use_result: Option<Value>,
    subtype: Option<String>,
    /// System lines' text.
    content: Option<Value>,
    /// An `api_error`'s details.
    error: Option<Value>,
    attachment: Option<RawAttachment>,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum LineKind {
    User,
    Assistant,
    System,
    Attachment,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct RawMessage {
    id: Option<String>,
    model: Option<String>,
    content: Option<Content>,
}

// ponytail: untagged buffers each content value (base64 images included)
// before matching; the largest local transcript (20 MB) loads in ~7 ms. Swap
// in a visitor `Deserialize` (`visit_str` / `visit_seq`) if loads get slow.
#[derive(Deserialize)]
#[serde(untagged)]
enum Content {
    Text(String),
    Blocks(Vec<RawBlock>),
}

#[derive(Deserialize)]
struct RawBlock {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
    thinking: Option<String>,
    id: Option<String>,
    name: Option<String>,
    input: Option<Value>,
    tool_use_id: Option<String>,
    is_error: Option<bool>,
    content: Option<Content>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAttachment {
    #[serde(rename = "type")]
    kind: String,
    command_mode: Option<String>,
    prompt: Option<Content>,
}

/// A transcript line kept for the rebuild.
#[derive(Debug)]
struct Entry {
    parent: Option<String>,
    /// Where a compaction boundary's history continues.
    logical_parent: Option<String>,
    body: Body,
}

/// What a kept line shows, with images and raw JSON already dropped.
#[derive(Debug)]
enum Body {
    Text {
        message_id: String,
        text: String,
    },
    Thinking {
        message_id: String,
        text: String,
    },
    ToolUse {
        message_id: String,
        id: String,
        name: String,
        input: Value,
    },
    Results(Vec<ToolResult>),
    Error(String),
    You(String),
    System(SystemLevel, String),
    Divider,
    Hidden,
}

impl Body {
    /// The API message an assistant line is part of.
    fn message_id(&self) -> Option<&str> {
        match self {
            Self::Text { message_id, .. }
            | Self::Thinking { message_id, .. }
            | Self::ToolUse { message_id, .. } => Some(message_id),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct ToolResult {
    tool_use_id: String,
    is_error: bool,
    output: String,
    /// Lines added and removed by an edit.
    diff: Option<(usize, usize)>,
}

/// A transcript read so far: its lines, the facts for the header, and how
/// many lines couldn't be read.
#[derive(Debug, Default)]
pub struct Conversation {
    /// In file order; only ever appended to.
    entries: Vec<Entry>,
    by_uuid: HashMap<String, usize>,
    skipped: usize,
    branch: Option<String>,
    model: Option<String>,
}

const LOCAL_COMMAND_TAGS: &[&str] = &[
    "<local-command-stdout>",
    "</local-command-stdout>",
    "<local-command-stderr>",
    "</local-command-stderr>",
];
const BASH_OUTPUT_TAGS: &[&str] = &[
    "<bash-stdout>",
    "</bash-stdout>",
    "<bash-stderr>",
    "</bash-stderr>",
];

impl Conversation {
    /// Adds complete transcript lines, in file order.
    pub fn push_lines(&mut self, text: &str) {
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            match serde_json::from_str::<RawLine>(line) {
                Ok(line) => self.push(line),
                Err(_) => self.skipped += 1,
            }
        }
    }

    /// Lines that couldn't be read.
    pub fn skipped(&self) -> usize {
        self.skipped
    }

    /// The git branch of the latest line that names one.
    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    /// The model ID of Claude's latest reply, as the transcript records it.
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    fn push(&mut self, line: RawLine) {
        if let Some(branch) = line.git_branch.as_ref().filter(|branch| !branch.is_empty()) {
            self.branch = Some(branch.clone());
        }
        if line.kind == LineKind::Assistant
            && let Some(model) = line
                .message
                .as_ref()
                .and_then(|message| message.model.as_deref())
                .filter(|&model| model != "<synthetic>")
        {
            self.model = Some(model.to_owned());
        }
        let Some(uuid) = line.uuid.clone() else {
            return;
        };
        if line.kind == LineKind::Other
            || line.is_sidechain == Some(true)
            || self.by_uuid.contains_key(&uuid)
        {
            return;
        }
        let entry = Entry {
            parent: line.parent_uuid.clone(),
            logical_parent: line.logical_parent_uuid.clone(),
            body: classify(line, &uuid),
        };
        self.by_uuid.insert(uuid, self.entries.len());
        self.entries.push(entry);
    }

    /// The newest branch as blocks, in file order. Paths under `cwd` are
    /// shown relative to it.
    pub fn blocks(&self, cwd: &Path) -> Vec<Block> {
        let included = self.included();
        let tool_ids: HashSet<&str> = self
            .entries
            .iter()
            .zip(&included)
            .filter_map(|(entry, &included)| match &entry.body {
                Body::ToolUse { id, .. } if included => Some(id.as_str()),
                _ => None,
            })
            .collect();
        let results = {
            let mut results: HashMap<&str, &ToolResult> = HashMap::new();
            for result in self.entries.iter().flat_map(|entry| match &entry.body {
                Body::Results(results) => results.as_slice(),
                _ => &[],
            }) {
                if tool_ids.contains(result.tool_use_id.as_str()) {
                    results.entry(&result.tool_use_id).or_insert(result);
                }
            }
            results
        };
        let mut blocks: Vec<Block> = Vec::new();
        let mut last_claude: Option<&str> = None;
        for (index, entry) in self
            .entries
            .iter()
            .enumerate()
            .filter(|&(index, _)| included.get(index) == Some(&true))
        {
            if let Body::Text { message_id, text } = &entry.body
                && last_claude == Some(message_id.as_str())
                && let Some(Block {
                    kind: BlockKind::Claude(merged),
                    parts,
                    ..
                }) = blocks.last_mut()
            {
                merged.push_str("\n\n");
                merged.push_str(text);
                *parts += 1;
                continue;
            }
            let (kind, parts) = match &entry.body {
                Body::Text { text, .. } => (BlockKind::Claude(text.clone()), 1),
                Body::Thinking { text, .. } => (BlockKind::Thinking(text.clone()), 1),
                Body::ToolUse {
                    id, name, input, ..
                } => {
                    let result = results.get(id.as_str()).copied();
                    let call = tool_call(name, input, result, cwd);
                    (BlockKind::Tool(call), 1 + u32::from(result.is_some()))
                }
                Body::Error(text) => (system(SystemLevel::Error, text), 1),
                Body::You(text) => (BlockKind::You(text.clone()), 1),
                Body::System(level, text) => (system(*level, text), 1),
                Body::Divider => (system(SystemLevel::Divider, "Conversation compacted"), 1),
                Body::Results(_) | Body::Hidden => continue,
            };
            last_claude = match &entry.body {
                Body::Text { message_id, .. } => Some(message_id),
                _ => None,
            };
            blocks.push(Block {
                id: BlockId(index as u32),
                parts,
                kind,
            });
        }
        blocks
    }

    /// Which entries the newest branch shows: the path back from the newest
    /// entry, plus every entry of an API message on that path.
    fn included(&self) -> Vec<bool> {
        let on_path = self.newest_path();
        let message_ids: HashSet<&str> = self
            .entries
            .iter()
            .zip(&on_path)
            .filter(|&(_, &on_path)| on_path)
            .filter_map(|(entry, _)| entry.body.message_id())
            .collect();
        self.entries
            .iter()
            .zip(&on_path)
            .map(|(entry, &on_path)| {
                on_path
                    || entry
                        .body
                        .message_id()
                        .is_some_and(|id| message_ids.contains(id))
            })
            .collect()
    }

    /// Marks the entries on the path back from the newest one. A parent
    /// that isn't in the file continues at the previous entry, and a
    /// compaction boundary continues at the history it summarized.
    fn newest_path(&self) -> Vec<bool> {
        let mut on_path = vec![false; self.entries.len()];
        let mut next = self.entries.len().checked_sub(1);
        while let Some(index) = next
            && let Some(entry) = self.entries.get(index)
            && let Some(seen) = on_path.get_mut(index)
            && !*seen
        {
            *seen = true;
            let parent = match (&entry.parent, &entry.body) {
                (Some(parent), _) => Some(parent),
                (None, Body::Divider) => entry.logical_parent.as_ref(),
                (None, _) => None,
            };
            next = parent.and_then(|uuid| {
                self.by_uuid
                    .get(uuid)
                    .copied()
                    .or_else(|| index.checked_sub(1))
            });
        }
        on_path
    }
}

fn system(level: SystemLevel, text: &str) -> BlockKind {
    BlockKind::System {
        level,
        text: text.to_owned(),
    }
}

/// What a line shows.
fn classify(line: RawLine, uuid: &str) -> Body {
    match line.kind {
        LineKind::Assistant => assistant_body(line, uuid),
        LineKind::User => user_body(line),
        LineKind::Attachment => match line.attachment {
            Some(RawAttachment {
                kind,
                command_mode: Some(mode),
                prompt: Some(prompt),
            }) if kind == "queued_command" && mode == "prompt" => user_text(&content_text(prompt)),
            _ => Body::Hidden,
        },
        LineKind::System => system_body(&line),
        LineKind::Other => Body::Hidden,
    }
}

/// An assistant line: one thinking, text, or tool-use block, or an API error.
fn assistant_body(line: RawLine, uuid: &str) -> Body {
    let Some(message) = line.message else {
        return Body::Hidden;
    };
    let mut blocks = blocks_of(message.content).into_iter();
    if line.is_api_error_message == Some(true) {
        let text = blocks
            .find(|block| block.kind == "text")
            .and_then(|block| kept(block.text))
            .unwrap_or_else(|| "API error".to_owned());
        return Body::Error(text);
    }
    if message.model.as_deref() == Some("<synthetic>") {
        return Body::Hidden;
    }
    let message_id = message.id.unwrap_or_else(|| uuid.to_owned());
    let Some(block) = blocks.next() else {
        return Body::Hidden;
    };
    match block.kind.as_str() {
        "text" => kept(block.text).map_or(Body::Hidden, |text| Body::Text { message_id, text }),
        "thinking" => {
            kept(block.thinking).map_or(Body::Hidden, |text| Body::Thinking { message_id, text })
        }
        "tool_use" => match (block.id, block.name) {
            (Some(id), Some(name)) => Body::ToolUse {
                message_id,
                id,
                name,
                input: block.input.unwrap_or(Value::Null),
            },
            _ => Body::Hidden,
        },
        _ => Body::Hidden,
    }
}

/// A user line: tool results, or text the user typed or Claude recorded.
fn user_body(line: RawLine) -> Body {
    if line.is_meta == Some(true) || line.is_compact_summary == Some(true) {
        return Body::Hidden;
    }
    match line.message.and_then(|message| message.content) {
        Some(Content::Blocks(blocks)) if blocks.iter().any(|block| block.kind == "tool_result") => {
            let diff = line.tool_use_result.as_ref().and_then(diff_of);
            Body::Results(
                blocks
                    .into_iter()
                    .filter(|block| block.kind == "tool_result")
                    .filter_map(|block| {
                        Some(ToolResult {
                            tool_use_id: block.tool_use_id?,
                            is_error: block.is_error == Some(true),
                            output: detab(
                                block.content.map(content_text).unwrap_or_default().trim(),
                            ),
                            diff,
                        })
                    })
                    .collect(),
            )
        }
        Some(content) => user_text(&content_text(content)),
        None => Body::Hidden,
    }
}

/// Text on a user line: a prompt, a command, shell input or output, an
/// interruption, or bookkeeping Claude wraps in tags.
fn user_text(text: &str) -> Body {
    let text = text.trim();
    match text {
        _ if text.contains("<command-name>") => {
            let name = between(text, "<command-name>", "</command-name>").unwrap_or_default();
            let args = between(text, "<command-args>", "</command-args>").unwrap_or_default();
            Body::You(detab(format!("{name} {args}").trim()))
        }
        _ if text.starts_with("<local-command-stdout>")
            || text.starts_with("<local-command-stderr>") =>
        {
            info(&strip_tags(text, LOCAL_COMMAND_TAGS))
        }
        _ if text.starts_with("<bash-input>") => Body::You(format!(
            "! {}",
            detab(between(text, "<bash-input>", "</bash-input>").unwrap_or_default())
        )),
        _ if text.starts_with("<bash-stdout>") || text.starts_with("<bash-stderr>") => {
            info(&strip_tags(text, BASH_OUTPUT_TAGS))
        }
        _ if text.starts_with("[Request interrupted") => {
            Body::System(SystemLevel::Info, "Interrupted".to_owned())
        }
        _ if text.starts_with('<') || text.is_empty() => Body::Hidden,
        _ => Body::You(detab(text)),
    }
}

/// A system line, by subtype; bookkeeping ones are hidden.
fn system_body(line: &RawLine) -> Body {
    let content = line
        .content
        .as_ref()
        .and_then(Value::as_str)
        .unwrap_or_default();
    match line.subtype.as_deref() {
        Some("api_error") => {
            let message = line.error.as_ref().and_then(|error| {
                error
                    .pointer("/message")
                    .and_then(Value::as_str)
                    .or_else(|| error.pointer("/error/message").and_then(Value::as_str))
            });
            Body::System(
                SystemLevel::Error,
                message.map_or_else(
                    || "API error".to_owned(),
                    |message| detab(&format!("API error: {message}")),
                ),
            )
        }
        // Either the command the user ran or its output, tagged as on user lines.
        Some("local_command") => user_text(content),
        Some("informational") => info(content),
        Some("compact_boundary") => Body::Divider,
        _ => Body::Hidden,
    }
}

/// An info line, or `Hidden` if there's nothing to say.
fn info(text: &str) -> Body {
    match text.trim() {
        "" => Body::Hidden,
        text => Body::System(SystemLevel::Info, detab(text)),
    }
}

/// The blocks of a message's content; plain text counts as one text block.
fn blocks_of(content: Option<Content>) -> Vec<RawBlock> {
    match content {
        Some(Content::Blocks(blocks)) => blocks,
        Some(Content::Text(text)) => vec![RawBlock {
            kind: "text".to_owned(),
            text: Some(text),
            thinking: None,
            id: None,
            name: None,
            input: None,
            tool_use_id: None,
            is_error: None,
            content: None,
        }],
        None => Vec::new(),
    }
}

/// Content as text: text blocks joined by lines, `[image]` for each image.
fn content_text(content: Content) -> String {
    match content {
        Content::Text(text) => text,
        Content::Blocks(blocks) => blocks
            .into_iter()
            .filter_map(|block| match block.kind.as_str() {
                "text" => block.text,
                "image" => Some("[image]".to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Text worth showing, trimmed, or `None` if it's blank.
fn kept(text: Option<String>) -> Option<String> {
    text.map(|text| detab(text.trim()))
        .filter(|text| !text.is_empty())
}

/// The text between the first `open` and the `close` after it.
fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let (_, rest) = text.split_once(open)?;
    rest.split_once(close).map(|(inside, _)| inside)
}

fn strip_tags(text: &str, tags: &[&str]) -> String {
    tags.iter()
        .fold(text.to_owned(), |text, tag| text.replace(tag, ""))
}

/// Tabs as spaces; the terminal drops control characters.
fn detab(text: &str) -> String {
    text.replace('\t', "    ")
}

/// Lines an edit added and removed, from a tool result's `toolUseResult`.
fn diff_of(result: &Value) -> Option<(usize, usize)> {
    let result = result.as_object()?;
    match result.get("structuredPatch").and_then(Value::as_array) {
        Some(hunks) if !hunks.is_empty() => Some(
            hunks
                .iter()
                .filter_map(|hunk| hunk.get("lines").and_then(Value::as_array))
                .flatten()
                .filter_map(Value::as_str)
                .fold((0, 0), |(added, removed), line| {
                    if line.starts_with('+') {
                        (added + 1, removed)
                    } else if line.starts_with('-') {
                        (added, removed + 1)
                    } else {
                        (added, removed)
                    }
                }),
        ),
        _ if result.get("type").and_then(Value::as_str) == Some("create") => Some((
            result
                .get("content")
                .and_then(Value::as_str)
                .map_or(0, |content| content.lines().count()),
            0,
        )),
        _ => None,
    }
}

/// A tool call's one-line summary, status, and output.
fn tool_call(name: &str, input: &Value, result: Option<&ToolResult>, cwd: &Path) -> ToolCall {
    let field = |key: &str| input.get(key).and_then(Value::as_str);
    let path = |key: &str| field(key).map(|path| relative(path, cwd));
    let summary = match name {
        "Bash" => {
            let mut lines = field("command").unwrap_or_default().lines();
            let first = lines.next().unwrap_or_default();
            match lines.next() {
                Some(_) => format!("$ {first} …"),
                None => format!("$ {first}"),
            }
        }
        "Edit" | "MultiEdit" | "Write" => {
            let summary = with_arg(name, path("file_path"));
            match result.and_then(|result| result.diff) {
                Some((added, removed)) => format!("{summary} +{added} −{removed}"),
                None => summary,
            }
        }
        "Read" => with_arg(name, path("file_path")),
        "Grep" | "Glob" => with_arg(name, field("pattern").map(str::to_owned)),
        _ => with_arg(
            name,
            input
                .as_object()
                .and_then(|fields| fields.values().find_map(Value::as_str))
                .and_then(|arg| arg.lines().next())
                .map(str::to_owned),
        ),
    };
    ToolCall {
        summary: detab(&summary),
        status: match result {
            None => ToolStatus::Pending,
            Some(result) if result.is_error => ToolStatus::Failed,
            Some(_) => ToolStatus::Ok,
        },
        output: result
            .map(|result| result.output.clone())
            .filter(|output| !output.is_empty()),
    }
}

fn with_arg(name: &str, arg: Option<String>) -> String {
    arg.map_or_else(|| name.to_owned(), |arg| format!("{name} {arg}"))
}

/// `path` relative to `cwd` when it's under it.
fn relative(path: &str, cwd: &Path) -> String {
    Path::new(path).strip_prefix(cwd).map_or_else(
        |_| path.to_owned(),
        |relative| relative.display().to_string(),
    )
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "tests propagate file failures with `?` and assert on the outcome"
)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::{self, Write};
    use std::path::Path;

    use serde_json::{Value, json};
    use tempfile::tempdir;

    use super::Conversation;
    use crate::feat::preview::block::{
        Block, BlockId, BlockKind, SystemLevel, ToolCall, ToolStatus,
    };
    use crate::feat::sessions::transcript::read_new_lines;

    const CWD: &str = "/work/demo";

    fn user(uuid: &str, parent: Option<&str>, content: &Value) -> Value {
        json!({
            "type": "user", "uuid": uuid, "parentUuid": parent,
            "message": {"role": "user", "content": content},
        })
    }

    fn assistant(uuid: &str, parent: Option<&str>, message_id: &str, block: &Value) -> Value {
        json!({
            "type": "assistant", "uuid": uuid, "parentUuid": parent,
            "message": {"id": message_id, "model": "claude-opus-5-5", "role": "assistant", "content": [block]},
        })
    }

    fn text(text: &str) -> Value {
        json!({"type": "text", "text": text})
    }

    fn tool_use(uuid: &str, parent: Option<&str>, id: &str, name: &str, input: &Value) -> Value {
        assistant(
            uuid,
            parent,
            &format!("msg-{uuid}"),
            &json!({"type": "tool_use", "id": id, "name": name, "input": input}),
        )
    }

    fn tool_result(
        uuid: &str,
        parent: &str,
        id: &str,
        is_error: bool,
        tool_use_result: Option<&Value>,
    ) -> Value {
        json!({
            "type": "user", "uuid": uuid, "parentUuid": parent,
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": id, "content": "output", "is_error": is_error},
            ]},
            "toolUseResult": tool_use_result,
        })
    }

    fn transcript(lines: &[Value]) -> String {
        let lines: Vec<String> = lines.iter().map(Value::to_string).collect();
        format!("{}\n", lines.join("\n"))
    }

    fn conversation(text: &str) -> Conversation {
        let mut conversation = Conversation::default();
        conversation.push_lines(text);
        conversation
    }

    fn rebuild(text: &str) -> Vec<Block> {
        conversation(text).blocks(Path::new(CWD))
    }

    fn blocks_of(lines: &[Value]) -> Vec<Block> {
        rebuild(&transcript(lines))
    }

    /// Each block as `kind: text`, to compare whole conversations.
    fn labels(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .map(|block| match &block.kind {
                BlockKind::You(text) => format!("you: {text}"),
                BlockKind::Claude(text) => format!("claude: {text}"),
                BlockKind::Thinking(text) => format!("thinking: {text}"),
                BlockKind::Tool(call) => format!("tool: {}", call.summary),
                BlockKind::System { level, text } => {
                    let level = match level {
                        SystemLevel::Info => "info",
                        SystemLevel::Error => "error",
                        SystemLevel::Divider => "divider",
                    };
                    format!("{level}: {text}")
                }
            })
            .collect()
    }

    fn first_tool(blocks: &[Block]) -> Option<&ToolCall> {
        blocks.iter().find_map(|block| match &block.kind {
            BlockKind::Tool(call) => Some(call),
            _ => None,
        })
    }

    #[rstest::rstest]
    fn lines_of_one_message_become_thinking_claude_and_tool_blocks_in_order() {
        // Given thinking, text, and tool-use lines sharing one message id.
        let lines = [
            assistant(
                "a",
                None,
                "m1",
                &json!({"type": "thinking", "thinking": "Look first", "signature": "sig"}),
            ),
            assistant("b", Some("a"), "m1", &text("Checking the tests.")),
            assistant(
                "c",
                Some("b"),
                "m1",
                &json!({"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "cargo test"}}),
            ),
        ];

        // When rebuilding.
        let blocks = blocks_of(&lines);

        // Then they're a Thinking, a Claude, and a Tool block, in that order.
        assert_eq!(
            labels(&blocks),
            [
                "thinking: Look first",
                "claude: Checking the tests.",
                "tool: $ cargo test"
            ],
            "one message's lines should become one block each"
        );
    }

    #[rstest::rstest]
    fn two_text_lines_of_one_message_merge_into_one_claude_block() {
        // Given two text lines of one message.
        let lines = [
            assistant("a", None, "m1", &text("Hello")),
            assistant("b", Some("a"), "m1", &text("world")),
        ];

        // When rebuilding.
        let blocks = blocks_of(&lines);

        // Then they're one Claude block made of two parts.
        assert_eq!(
            blocks,
            [Block {
                id: BlockId(0),
                parts: 2,
                kind: BlockKind::Claude("Hello\n\nworld".to_owned()),
            }],
            "one message's text should be one block"
        );
    }

    #[rstest::rstest]
    fn rewind_shows_only_the_newest_branch() {
        // Given a real transcript where one prompt was sent again from the same parent.
        // When rebuilding it.
        let blocks = rebuild(include_str!("fixtures/rewind.jsonl"));

        // Then only the newest branch's prompt shows.
        let prompts: Vec<String> = labels(&blocks)
            .into_iter()
            .filter(|label| label.starts_with("you: "))
            .collect();
        assert_eq!(
            prompts,
            ["you: Write a limerick about parsers"],
            "the abandoned branch should be left out"
        );
    }

    #[rstest::rstest]
    fn parallel_tool_calls_each_get_their_own_result() {
        // Given a real excerpt where a tool result points at its own call,
        // not the line before it.
        // When rebuilding it.
        let blocks = rebuild(include_str!("fixtures/parallel_tools.jsonl"));

        // Then every call shows, each with its own output.
        let calls: Vec<(String, Option<String>)> = blocks
            .iter()
            .filter_map(|block| match &block.kind {
                BlockKind::Tool(call) => Some((call.summary.clone(), call.output.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            calls,
            [
                (
                    "$ cargo bench --bench parse".to_owned(),
                    Some("parse: 1 ms".to_owned())
                ),
                (
                    "$ cargo bench --bench write".to_owned(),
                    Some("write: 1 ms".to_owned())
                ),
                (
                    "$ cargo bench --bench read".to_owned(),
                    Some("read: 1 ms".to_owned())
                ),
            ],
            "parallel calls shouldn't be dropped or lose their results"
        );
    }

    #[rstest::rstest]
    fn missing_parent_continues_at_the_previous_line() {
        // Given a prompt whose parent isn't in the file.
        let lines = [
            user("a", None, &json!("First")),
            assistant("b", Some("a"), "m1", &text("Reply")),
            user("c", Some("gone"), &json!("Second")),
            assistant("d", Some("c"), "m2", &text("Done")),
        ];

        // When rebuilding.
        let blocks = blocks_of(&lines);

        // Then the blocks before it still show.
        assert_eq!(
            labels(&blocks),
            ["you: First", "claude: Reply", "you: Second", "claude: Done"],
            "a broken link shouldn't cut the history"
        );
    }

    #[rstest::rstest]
    fn repeated_uuid_keeps_its_first_copy() {
        // Given a prompt re-appended with the same uuid.
        let lines = [
            user("a", None, &json!("First")),
            user("a", None, &json!("Changed")),
        ];

        // When rebuilding.
        let blocks = blocks_of(&lines);

        // Then it's one block, from the first copy.
        assert_eq!(
            labels(&blocks),
            ["you: First"],
            "a repeated uuid should keep its first copy"
        );
    }

    #[rstest::rstest]
    fn compaction_shows_history_then_a_divider() {
        // Given a transcript that was compacted.
        // When rebuilding it.
        let blocks = rebuild(include_str!("fixtures/compaction.jsonl"));

        // Then the history before the boundary shows, then a divider, then what followed.
        assert_eq!(
            labels(&blocks),
            [
                "you: Fix the CSV parser",
                "claude: Fixed it.",
                "divider: Conversation compacted",
                "you: Now add a test",
                "claude: Added one.",
            ],
            "the rebuild should continue across the compaction boundary"
        );
    }

    #[rstest::rstest]
    fn unknown_line_type_is_ignored() {
        // Given an unknown line type between two known lines.
        let lines = [
            user("a", None, &json!("First")),
            json!({"type": "worktree-state", "uuid": "w", "parentUuid": "a", "worktree": {}}),
            assistant("b", Some("a"), "m1", &text("Reply")),
        ];

        // When reading them.
        let conversation = conversation(&transcript(&lines));

        // Then it adds no block and doesn't count as unreadable.
        assert_eq!(
            (
                labels(&conversation.blocks(Path::new(CWD))),
                conversation.skipped()
            ),
            (vec!["you: First".to_owned(), "claude: Reply".to_owned()], 0),
            "unknown line types should be ignored"
        );
    }

    #[rstest::rstest]
    fn malformed_line_is_counted_and_skipped() {
        // Given a malformed line before a prompt.
        let text = format!(
            "{{\"type\":\"user\",\"mess\n{}",
            transcript(&[user("a", None, &json!("First"))])
        );

        // When reading them.
        let conversation = conversation(&text);

        // Then it's counted and the prompt still shows.
        assert_eq!(
            (
                labels(&conversation.blocks(Path::new(CWD))),
                conversation.skipped()
            ),
            (vec!["you: First".to_owned()], 1),
            "a bad line should be skipped and counted"
        );
    }

    #[rstest::rstest]
    fn partial_last_line_is_parsed_once_after_its_completed() -> io::Result<()> {
        // Given a transcript read while its last line was still being written,
        // which is then completed.
        let dir = tempdir()?;
        let path = dir.path().join("session.jsonl");
        let second = user("b", Some("a"), &json!("Second")).to_string();
        let (head, tail) = second.split_at(10);
        fs::write(
            &path,
            format!("{}{head}", transcript(&[user("a", None, &json!("First"))])),
        )?;
        let mut conversation = Conversation::default();
        let first = read_new_lines(&path, 0)?;
        conversation.push_lines(&first.text);
        OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(format!("{tail}\n").as_bytes())?;

        // When reading again from the first read's offset.
        conversation.push_lines(&read_new_lines(&path, first.offset)?.text);

        // Then the completed line's block shows once.
        assert_eq!(
            labels(&conversation.blocks(Path::new(CWD))),
            ["you: First", "you: Second"],
            "the partial line should be parsed once, after it's complete"
        );
        Ok(())
    }

    #[rstest::rstest]
    fn empty_thinking_adds_no_block() {
        // Given a thinking line without text.
        let lines = [assistant(
            "a",
            None,
            "m1",
            &json!({"type": "thinking", "thinking": "", "signature": "sig"}),
        )];

        // When rebuilding.
        let blocks = blocks_of(&lines);

        // Then there's nothing to show.
        assert_eq!(blocks, [], "empty thinking should be hidden");
    }

    #[rstest::rstest]
    fn thinking_with_text_is_a_thinking_block() {
        // Given a thinking line with text.
        let lines = [assistant(
            "a",
            None,
            "m1",
            &json!({"type": "thinking", "thinking": "The last line is dropped.", "signature": "sig"}),
        )];

        // When rebuilding.
        let blocks = blocks_of(&lines);

        // Then it's a Thinking block.
        assert_eq!(
            labels(&blocks),
            ["thinking: The last line is dropped."],
            "non-empty thinking should show"
        );
    }

    #[rstest::rstest]
    fn queued_prompt_attachment_is_a_you_block() {
        // Given a prompt the user typed while Claude was working.
        let lines = [json!({
            "type": "attachment", "uuid": "a", "parentUuid": null,
            "attachment": {"type": "queued_command", "prompt": "Use criterion", "commandMode": "prompt"},
        })];

        // When rebuilding.
        let blocks = blocks_of(&lines);

        // Then it's a You block with the prompt.
        assert_eq!(
            labels(&blocks),
            ["you: Use criterion"],
            "queued prompts should show"
        );
    }

    #[rstest::rstest]
    fn slash_command_is_a_you_block_with_its_args() {
        // Given a slash command line.
        let lines = [user(
            "a",
            None,
            &json!(
                "<command-message>plan</command-message>\n<command-name>/plan</command-name>\n<command-args>M3</command-args>"
            ),
        )];

        // When rebuilding.
        let blocks = blocks_of(&lines);

        // Then it's a You block with the command and its args.
        assert_eq!(
            labels(&blocks),
            ["you: /plan M3"],
            "commands should show as typed"
        );
    }

    #[rstest::rstest]
    fn local_command_output_is_an_info_block() {
        // Given a local command's output line.
        let lines = [user(
            "a",
            None,
            &json!("<local-command-stdout>ok</local-command-stdout>"),
        )];

        // When rebuilding.
        let blocks = blocks_of(&lines);

        // Then it's an info block with the output.
        assert_eq!(labels(&blocks), ["info: ok"], "command output should show");
    }

    #[rstest::rstest]
    fn interruption_is_an_info_block() {
        // Given the line Claude writes when the user interrupts.
        let lines = [user(
            "a",
            None,
            &json!([text("[Request interrupted by user]")]),
        )];

        // When rebuilding.
        let blocks = blocks_of(&lines);

        // Then it's an info block saying so.
        assert_eq!(
            labels(&blocks),
            ["info: Interrupted"],
            "interruptions should show"
        );
    }

    #[rstest::rstest]
    #[case::meta(json!({
        "type": "user", "uuid": "a", "parentUuid": null, "isMeta": true,
        "message": {"content": "Base directory for this skill"},
    }))]
    #[case::task_notification(user("a", None, &json!("<task-notification>\n<task-id>t1</task-id>\n</task-notification>")))]
    #[case::local_command_caveat(user("a", None, &json!("<local-command-caveat>Caveat: do not respond</local-command-caveat>")))]
    fn bookkeeping_user_lines_add_no_block(#[case] line: Value) {
        // Given a user line that isn't something the user typed.
        // When rebuilding.
        let blocks = blocks_of(&[line]);

        // Then there's nothing to show.
        assert_eq!(blocks, [], "bookkeeping user lines should be hidden");
    }

    #[rstest::rstest]
    #[case::bash("Bash", json!({"command": "cargo test"}), None, "$ cargo test")]
    #[case::multi_line_bash("Bash", json!({"command": "cargo test\necho done"}), None, "$ cargo test …")]
    #[case::read("Read", json!({"file_path": "/work/demo/src/lib.rs"}), None, "Read src/lib.rs")]
    #[case::grep("Grep", json!({"pattern": "fn parse", "path": "/work/demo"}), None, "Grep fn parse")]
    #[case::glob("Glob", json!({"pattern": "**/*.rs"}), None, "Glob **/*.rs")]
    #[case::other_tool(
        "WebFetch",
        json!({"url": "https://example.com/docs", "prompt": "Summarize"}),
        None,
        "WebFetch https://example.com/docs"
    )]
    #[case::edit_with_a_patch(
        "Edit",
        json!({"file_path": "/work/demo/src/lib.rs", "old_string": "old", "new_string": "new"}),
        Some(json!({"structuredPatch": [{"oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2, "lines": [" keep", "-old", "+new"]}]})),
        "Edit src/lib.rs +1 −1"
    )]
    #[case::write_create(
        "Write",
        json!({"file_path": "/work/demo/tests/a.rs", "content": "one\ntwo\n"}),
        Some(json!({"type": "create", "filePath": "/work/demo/tests/a.rs", "content": "one\ntwo\n", "structuredPatch": []})),
        "Write tests/a.rs +2 −0"
    )]
    fn tool_call_is_summarized_on_one_line(
        #[case] name: &str,
        #[case] input: Value,
        #[case] tool_use_result: Option<Value>,
        #[case] expected: &str,
    ) {
        // Given a tool call and its result.
        let lines = [
            tool_use("a", None, "t1", name, &input),
            tool_result("b", "a", "t1", false, tool_use_result.as_ref()),
        ];

        // When rebuilding.
        let blocks = blocks_of(&lines);

        // Then its summary names the tool and its main argument.
        assert_eq!(
            first_tool(&blocks).map(|call| call.summary.as_str()),
            Some(expected),
            "the tool call's one-liner"
        );
    }

    #[rstest::rstest]
    fn failed_tool_result_marks_the_call_failed() {
        // Given a tool call whose result is an error.
        let lines = [
            tool_use("a", None, "t1", "Bash", &json!({"command": "cargo clippy"})),
            tool_result("b", "a", "t1", true, Some(&json!("Error: Exit code 1"))),
        ];

        // When rebuilding.
        let blocks = blocks_of(&lines);

        // Then the call failed.
        assert_eq!(
            first_tool(&blocks).map(|call| call.status),
            Some(ToolStatus::Failed),
            "an error result should fail the call"
        );
    }

    #[rstest::rstest]
    fn tool_call_without_a_result_is_pending() {
        // Given a tool call whose result hasn't arrived.
        let lines = [tool_use(
            "a",
            None,
            "t1",
            "Bash",
            &json!({"command": "cargo test"}),
        )];

        // When rebuilding.
        let blocks = blocks_of(&lines);

        // Then it's pending, with no output.
        assert_eq!(
            first_tool(&blocks).map(|call| (call.status, call.output.clone())),
            Some((ToolStatus::Pending, None)),
            "a call without a result should be pending"
        );
    }

    #[rstest::rstest]
    fn synthetic_api_error_line_is_an_error_block() {
        // Given the line Claude writes when the API fails mid-response.
        let lines = [json!({
            "type": "assistant", "uuid": "a", "parentUuid": null, "isApiErrorMessage": true,
            "message": {"id": "m1", "model": "<synthetic>", "content": [text("API Error: Connection lost mid-response")]},
        })];

        // When rebuilding.
        let blocks = blocks_of(&lines);

        // Then it's an error block with the message.
        assert_eq!(
            labels(&blocks),
            ["error: API Error: Connection lost mid-response"],
            "API errors should show"
        );
    }

    #[rstest::rstest]
    #[case::turn_duration(json!({"type": "system", "subtype": "turn_duration", "durationMs": 1000}), false)]
    #[case::away_summary(json!({"type": "system", "subtype": "away_summary", "content": "While you were away"}), false)]
    #[case::informational(json!({"type": "system", "subtype": "informational", "content": "Update available", "level": "info"}), true)]
    #[case::local_command(json!({"type": "system", "subtype": "local_command", "content": "<local-command-stdout>Done</local-command-stdout>"}), true)]
    #[case::api_error(json!({"type": "system", "subtype": "api_error", "level": "error", "error": {"message": "Overloaded"}}), true)]
    fn system_line_is_shown_only_when_it_tells_the_user_something(
        #[case] line: Value,
        #[case] shown: bool,
    ) {
        // Given a system line of some subtype.
        let line = {
            let mut line = line;
            if let Some(fields) = line.as_object_mut() {
                fields.insert("uuid".to_owned(), json!("s"));
                fields.insert("parentUuid".to_owned(), Value::Null);
            }
            line
        };

        // When rebuilding.
        let blocks = blocks_of(&[line]);

        // Then it shows or not, by subtype.
        assert_eq!(!blocks.is_empty(), shown, "system subtypes shown or hidden");
    }

    #[rstest::rstest]
    fn branch_is_the_latest_line_that_names_one() {
        // Given lines on `main`, then `fix`, then one with no branch.
        let lines = [
            json!({"type": "user", "uuid": "a", "gitBranch": "main", "message": {"content": "First"}}),
            json!({"type": "user", "uuid": "b", "parentUuid": "a", "gitBranch": "fix", "message": {"content": "Second"}}),
            json!({"type": "user", "uuid": "c", "parentUuid": "b", "gitBranch": "", "message": {"content": "Third"}}),
        ];

        // When reading them.
        let conversation = conversation(&transcript(&lines));

        // Then the branch is `fix`.
        assert_eq!(
            conversation.branch(),
            Some("fix"),
            "the latest named branch"
        );
    }

    #[rstest::rstest]
    fn model_is_the_latest_real_model_id() {
        // Given a reply from claude-opus-5-5, then a synthetic one.
        let lines = [
            assistant("a", None, "m1", &text("Done")),
            json!({
                "type": "assistant", "uuid": "b", "parentUuid": "a",
                "message": {"id": "m2", "model": "<synthetic>", "content": [text("No response requested.")]},
            }),
        ];

        // When reading them.
        let conversation = conversation(&transcript(&lines));

        // Then the model is the real one's ID.
        assert_eq!(
            conversation.model(),
            Some("claude-opus-5-5"),
            "the latest real model"
        );
    }

    #[rstest::rstest]
    fn real_session_rebuilds_into_its_blocks() {
        // Given a real short session: prompts, thinking, replies, tool calls
        // (one failing), a slash command, local command output, a queued
        // prompt, an interrupt, and bookkeeping lines.
        // When rebuilding it.
        let blocks = rebuild(include_str!("fixtures/session.jsonl"));

        // Then every visible line shows, in order, and bookkeeping doesn't.
        assert_eq!(
            labels(&blocks),
            [
                "you: Fix the CSV parser\nIt drops the last line",
                "claude: Sure. Let me look at the parser first.",
                "you: Go ahead",
                "thinking: The last line has no trailing newline, so the loop exits before it.",
                "tool: Edit src/csv.rs +1 −1",
                "tool: $ cargo test",
                "tool: $ cargo clippy -- -D warnings …",
                "tool: Grep fn parse",
                "tool: Read src/csv.rs",
                "tool: Write tests/csv.rs +2 −0",
                "claude: Fixed: the parser keeps the last line, and a test covers it.",
                "you: /plan M3",
                "info: Plan saved",
                "you: Now add a benchmark",
                "tool: $ cargo bench",
                "you: Use criterion",
                "info: Interrupted",
                "you: /context",
                "info: Context usage: 12%",
            ],
            "the session's blocks"
        );
    }
}
