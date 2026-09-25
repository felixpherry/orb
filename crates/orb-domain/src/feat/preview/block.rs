//! The preview's blocks — one prompt, reply, thought, tool call, or system
//! message each.

/// Identifies a block while its transcript grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub u32);

/// One thing in the conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub id: BlockId,
    /// Transcript lines that make it up; changes when its content does.
    pub parts: u32,
    pub kind: BlockKind,
}

/// What a block holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    /// A prompt, `/command args`, or `! shell command`.
    You(String),
    /// Claude's reply text, as markdown.
    Claude(String),
    Thinking(String),
    Tool(ToolCall),
    System {
        level: SystemLevel,
        text: String,
    },
}

/// How a system block reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemLevel {
    Info,
    Error,
    /// Where the conversation was compacted.
    Divider,
}

/// A tool Claude called, and its result once it arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    /// The one-line summary, e.g. `$ cargo test`.
    pub summary: String,
    pub status: ToolStatus,
    /// The result's text; `None` until the result arrives.
    pub output: Option<String>,
}

/// Where a tool call stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    /// No result yet.
    Pending,
    Ok,
    Failed,
}

impl Block {
    /// Tool calls with output and thinking fold.
    pub fn foldable(&self) -> bool {
        match &self.kind {
            BlockKind::Tool(call) => call.output.is_some(),
            BlockKind::Thinking(_) => true,
            BlockKind::You(_) | BlockKind::Claude(_) | BlockKind::System { .. } => false,
        }
    }

    /// What `y` copies: a tool call's summary and output, Claude's markdown,
    /// and every other block's text.
    pub fn raw_text(&self) -> String {
        match &self.kind {
            BlockKind::Tool(ToolCall {
                summary,
                output: Some(output),
                ..
            }) => format!("{summary}\n{output}"),
            BlockKind::Tool(ToolCall { summary, .. }) => summary.clone(),
            BlockKind::You(text)
            | BlockKind::Claude(text)
            | BlockKind::Thinking(text)
            | BlockKind::System { text, .. } => text.clone(),
        }
    }
}
