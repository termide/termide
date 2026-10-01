//! The chat as a Markdown file: what the user wrote and what the agent
//! answered, each under its speaker and time, the days as headings of their
//! own. Reasoning, tool calls, their results and the panel's own notices are
//! left out — the file is the conversation, not the session's working.

use termide_agent_core::{
    AssistantContent, EntryKind, Message, Session, UserContent, DEFAULT_AGENT,
};
use termide_core::PanelEvent;

use crate::transcript::Item;
use crate::{capitalize, AgentPanel, NoticeKind};

/// The speaker's glyph in a saved heading: the panel's own `🤖` for the
/// agent, a person for the user. Kept out of the translations: an emoji
/// says the same in every language.
const AGENT_MARK: &str = "🤖";
const YOU_MARK: &str = "🧑";

/// One message of the chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChatMessage {
    /// Whether the user or the agent wrote it, which picks the glyph.
    pub user: bool,
    /// Who wrote it, as the heading shows it: `You`, or the agent's name —
    /// a custom agent's own, the panel's `Agent` label for the default one.
    pub who: String,
    pub when: When,
    pub text: String,
}

/// When a message was written: a timestamp from the session log, or only
/// the clock time the transcript keeps when there is no log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum When {
    Millis(u64),
    Clock(String),
}

/// The name a saved heading shows for the agent: a custom agent's own name
/// capitalized, as the panel's title spells it, the `Agent` label for the
/// default one.
fn agent_who(agent: Option<&str>) -> String {
    match agent {
        Some(name) if name != DEFAULT_AGENT => capitalize(name),
        _ => termide_i18n::t().panel_agent().to_string(),
    }
}

/// The chat on the session's current branch, compactions and all: a
/// compaction shortens what the model sees, not what was said. Each answer
/// is ascribed to the agent the branch ran as there, so a session that
/// switched agents reads right.
pub(crate) fn chat_from_session(session: &Session) -> Vec<ChatMessage> {
    let t = termide_i18n::t();
    let mut agent: Option<String> = None;
    session
        .branch()
        .into_iter()
        .filter_map(|entry| {
            let message = match &entry.kind {
                // Whom the branch runs as from here: an answer is ascribed to
                // the agent in force when it was written.
                EntryKind::AgentChange { agent: name } => {
                    agent = Some(name.clone());
                    return None;
                }
                EntryKind::Message { message, .. } => message,
                _ => return None,
            };
            let (user, text) = match message {
                // What the user typed: the `/name args` a template or skill
                // was expanded from, not the expansion.
                Message::User(user) => (
                    true,
                    user.command.clone().unwrap_or_else(|| {
                        user.content
                            .iter()
                            .map(|UserContent::Text { text }| text.as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    }),
                ),
                Message::Assistant(assistant) => (
                    false,
                    assistant
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            AssistantContent::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                ),
                Message::ToolResult(_) => return None,
            };
            let text = text.trim().to_string();
            (!text.is_empty()).then_some(ChatMessage {
                user,
                who: if user {
                    t.agent_export_you().to_string()
                } else {
                    agent_who(agent.as_deref())
                },
                when: When::Millis(entry.timestamp),
                text,
            })
        })
        .collect()
}

/// The chat as the transcript shows it, for a panel with no session log.
/// Every answer is ascribed to `agent`, the agent the panel runs.
pub(crate) fn chat_from_transcript(items: &[Item], agent: &str) -> Vec<ChatMessage> {
    let t = termide_i18n::t();
    items
        .iter()
        .filter_map(|item| {
            let (user, text, at) = match item {
                Item::User { text, at, command } => (true, command.as_ref().unwrap_or(text), at),
                Item::Assistant { text, at, .. } => (false, text, at),
                _ => return None,
            };
            let text = text.trim().to_string();
            (!text.is_empty()).then_some(ChatMessage {
                user,
                who: if user {
                    t.agent_export_you().to_string()
                } else {
                    agent_who(Some(agent))
                },
                when: When::Clock(at.clone()),
                text,
            })
        })
        .collect()
}

/// The Markdown of a chat: a title, then the day as a heading whenever the
/// day changes, then each message under a heading with the speaker's glyph,
/// their name and the time.
pub(crate) fn chat_markdown(title: &str, chat: &[ChatMessage]) -> String {
    use chrono::TimeZone;
    let mut out = format!("# {title}\n");
    let mut day: Option<String> = None;
    for message in chat {
        let (date, time) = match &message.when {
            When::Millis(ms) => match chrono::Local.timestamp_millis_opt(*ms as i64) {
                chrono::offset::LocalResult::Single(at) => (
                    Some(at.format("%Y-%m-%d").to_string()),
                    at.format("%H:%M").to_string(),
                ),
                _ => (None, String::new()),
            },
            // A transcript without a session log keeps the clock time only:
            // there is no day to head with.
            When::Clock(clock) => (None, clock.get(..5).unwrap_or(clock).to_string()),
        };
        if let Some(date) = &date {
            if day.as_deref() != Some(date.as_str()) {
                day = Some(date.clone());
                out.push_str(&format!("\n## {date}\n"));
            }
        }
        let mark = if message.user { YOU_MARK } else { AGENT_MARK };
        if time.is_empty() {
            out.push_str(&format!("\n### {mark} {}\n\n", message.who));
        } else {
            out.push_str(&format!("\n### {mark} {} · {time}\n\n", message.who));
        }
        out.push_str(&message.text);
        out.push('\n');
    }
    out
}

/// `text` made safe as a file name: separators, reserved and control
/// characters become spaces, whitespace collapses, at most 80 characters.
fn file_name_part(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let joined = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.trim_matches('.').trim().chars().take(80).collect()
}

impl AgentPanel {
    /// `Ctrl+S` and the `[≡]` menu: the chat to a Markdown file through the
    /// Save As dialog, named after the session.
    pub(crate) fn save_chat(&mut self) -> Vec<PanelEvent> {
        let t = termide_i18n::t();
        let chat = match &self.session {
            Some(session) => chat_from_session(session),
            None => chat_from_transcript(self.transcript.items(), &self.agent),
        };
        if chat.is_empty() {
            self.notice(t.agent_notice_chat_empty(), NoticeKind::Info);
            return vec![PanelEvent::NeedsRedraw];
        }
        let name = self
            .session
            .as_ref()
            .and_then(Session::name)
            .map(file_name_part)
            .filter(|name| !name.is_empty());
        let title = name
            .clone()
            .unwrap_or_else(|| t.agent_export_title().to_string());
        let default_name = match name {
            Some(name) => format!("{name}.md"),
            None => format!("chat-{}.md", chrono::Local::now().format("%Y-%m-%d-%H%M")),
        };
        vec![PanelEvent::SaveContentAs {
            content: chat_markdown(&title, &chat),
            default_name,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chat_reads_as_days_then_messages_under_who_and_when() {
        use chrono::TimeZone;
        let at = |h: u32, m: u32, day: u32| {
            chrono::Local
                .with_ymd_and_hms(2026, 10, day, h, m, 0)
                .unwrap()
                .timestamp_millis() as u64
        };
        let msg = |user: bool, who: &str, when: When, text: &str| ChatMessage {
            user,
            who: who.into(),
            when,
            text: text.into(),
        };
        let chat = vec![
            msg(true, "You", When::Millis(at(21, 3, 1)), "hello"),
            msg(false, "Agent", When::Millis(at(21, 4, 1)), "**hi**"),
            msg(true, "You", When::Millis(at(9, 0, 2)), "next day"),
        ];
        assert_eq!(
            chat_markdown("Fix the build", &chat),
            "# Fix the build\n\n## 2026-10-01\n\n### 🧑 You · 21:03\n\nhello\n\n### 🤖 Agent · 21:04\n\n**hi**\n\n## 2026-10-02\n\n### 🧑 You · 09:00\n\nnext day\n"
        );
        // A clock time with no date to head a day with: the time alone.
        assert_eq!(
            chat_markdown(
                "c",
                &[msg(false, "Reviewer", When::Clock("10:00:05".into()), "ok")]
            ),
            "# c\n\n### 🤖 Reviewer · 10:00\n\nok\n"
        );
    }

    #[test]
    fn only_what_was_said_comes_from_the_session_log() {
        use termide_agent_core::{
            AssistantMessage, StopReason, ToolCall, ToolResultMessage, Usage, UserMessage,
        };
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), dir.path()).unwrap();
        let answer = |content: Vec<AssistantContent>| AssistantMessage {
            content,
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            provider: "p".into(),
            model: "m".into(),
            error_message: None,
            timestamp: 0,
        };
        let call = ToolCall {
            id: "c1".into(),
            name: "read".into(),
            arguments: serde_json::json!({ "path": "a" }),
            extra_content: None,
        };
        let messages = [
            Message::User(UserMessage::text("look at a")),
            Message::Assistant(answer(vec![
                AssistantContent::thinking("hmm"),
                AssistantContent::Text {
                    text: "Reading it.".into(),
                },
                AssistantContent::ToolCall(call.clone()),
            ])),
            Message::ToolResult(ToolResultMessage::text(&call, "contents")),
            // A turn that only calls a tool says nothing.
            Message::Assistant(answer(vec![AssistantContent::ToolCall(call.clone())])),
            Message::Assistant(answer(vec![AssistantContent::Text {
                text: "It is fine.".into(),
            }])),
            Message::User(
                UserMessage::text("the expanded skill").with_command(Some("/deploy".into())),
            ),
        ];
        for message in &messages {
            session.append_message(message).unwrap();
        }
        let said: Vec<(bool, String, String)> = chat_from_session(&session)
            .into_iter()
            .map(|m| (m.user, m.who, m.text))
            .collect();
        assert_eq!(
            said,
            [
                (true, "You".to_string(), "look at a".to_string()),
                (false, "Agent".to_string(), "Reading it.".to_string()),
                (false, "Agent".to_string(), "It is fine.".to_string()),
                (true, "You".to_string(), "/deploy".to_string()),
            ]
        );
    }

    #[test]
    fn an_answer_carries_the_agent_the_session_ran_as() {
        use termide_agent_core::{AssistantMessage, Message, StopReason, Usage, UserMessage};
        let answer_text = |text: &str| {
            Message::Assistant(AssistantMessage {
                content: vec![AssistantContent::Text { text: text.into() }],
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                provider: "p".into(),
                model: "m".into(),
                error_message: None,
                timestamp: 0,
            })
        };
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), dir.path()).unwrap();
        session.append_agent_change(DEFAULT_AGENT).unwrap();
        session
            .append_message(&Message::User(UserMessage::text("a")))
            .unwrap();
        session.append_message(&answer_text("mine")).unwrap();
        session.append_agent_change("web-dev").unwrap();
        session.append_message(&answer_text("theirs")).unwrap();
        let said: Vec<(bool, String)> = chat_from_session(&session)
            .into_iter()
            .map(|m| (m.user, m.who))
            .collect();
        assert_eq!(
            said,
            [
                (true, "You".to_string()),
                (false, "Agent".to_string()),
                (false, "Web-dev".to_string()),
            ]
        );
    }

    #[test]
    fn only_what_was_said_comes_from_the_transcript() {
        let items = vec![
            Item::Notice {
                text: "mcp github: 3 tools connected".into(),
                kind: NoticeKind::Info,
            },
            Item::User {
                text: "expanded template".into(),
                at: "10:00:01".into(),
                command: Some("/review src".into()),
            },
            Item::Thinking {
                text: "pondering".into(),
                streaming: false,
                at: "10:00:02".into(),
                cost: None,
            },
            Item::Assistant {
                text: "Looks fine.".into(),
                streaming: false,
                error: None,
                at: "10:00:05".into(),
                cost: None,
                run_ms: None,
            },
        ];
        assert_eq!(
            chat_from_transcript(&items, DEFAULT_AGENT),
            [
                ChatMessage {
                    user: true,
                    who: "You".into(),
                    when: When::Clock("10:00:01".into()),
                    text: "/review src".into(),
                },
                ChatMessage {
                    user: false,
                    who: "Agent".into(),
                    when: When::Clock("10:00:05".into()),
                    text: "Looks fine.".into(),
                },
            ]
        );
        // A custom agent is named in the answer's place.
        assert_eq!(
            chat_from_transcript(&items, "reviewer")[1].who,
            "Reviewer".to_string()
        );
        assert_eq!(file_name_part("a/b: c?"), "a b c");
    }
}
