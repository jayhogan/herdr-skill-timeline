//! Turns Claude Code transcript lines into skill events.
//!
//! Two ways a skill shows up in a transcript:
//! - Claude calls it: an `assistant` entry with a `tool_use` block named `Skill`.
//! - The user types `/name`: a `user` entry containing
//!   `<command-name>/name</command-name>`, confirmed by a later `isMeta` user
//!   entry starting with "Base directory for this skill:". Built-in commands
//!   (`/clear`, `/model`, ...) never get that confirmation.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Via {
    Tool,
    Slash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEvent {
    pub name: String,
    pub timestamp: DateTime<Utc>,
    pub via: Via,
}

const SKILL_META_PREFIX: &str = "Base directory for this skill:";

/// Stateful line-by-line parser for one transcript. Feed lines in file order.
#[derive(Debug, Default)]
pub struct Parser {
    /// A typed `/command` waiting for its skill confirmation.
    pending: Option<(String, DateTime<Utc>)>,
    /// `tool_use` ids already emitted (resumed sessions can repeat history).
    seen_tool_ids: HashSet<String>,
}

impl Parser {
    pub fn feed(&mut self, line: &str) -> Vec<SkillEvent> {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            return vec![];
        };
        if entry.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return vec![];
        }
        let Some(timestamp) = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc))
        else {
            return vec![];
        };
        let content = entry.pointer("/message/content");

        match entry.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                self.pending = None;
                self.skill_tool_uses(content, timestamp)
            }
            Some("user") => {
                let text = text_of(content);
                if entry.get("isMeta").and_then(Value::as_bool) == Some(true) {
                    if text.trim_start().starts_with(SKILL_META_PREFIX)
                        && let Some((name, at)) = self.pending.take()
                    {
                        return vec![SkillEvent {
                            name,
                            timestamp: at,
                            via: Via::Slash,
                        }];
                    }
                    // Other meta entries (hook output etc.) leave `pending` alone.
                    return vec![];
                }
                self.pending = command_name(&text).map(|name| (name, timestamp));
                vec![]
            }
            _ => vec![],
        }
    }

    fn skill_tool_uses(&mut self, content: Option<&Value>, ts: DateTime<Utc>) -> Vec<SkillEvent> {
        let Some(blocks) = content.and_then(Value::as_array) else {
            return vec![];
        };
        let mut events = vec![];
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_use")
                || block.get("name").and_then(Value::as_str) != Some("Skill")
            {
                continue;
            }
            let Some(name) = block.pointer("/input/skill").and_then(Value::as_str) else {
                continue;
            };
            if let Some(id) = block.get("id").and_then(Value::as_str)
                && !self.seen_tool_ids.insert(id.to_string())
            {
                continue;
            }
            events.push(SkillEvent {
                name: name.to_string(),
                timestamp: ts,
                via: Via::Tool,
            });
        }
        events
    }
}

/// Message content as plain text: either a string, or the concatenated
/// `text` of its text blocks.
fn text_of(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// `x` from `<command-name>/x</command-name>`, if present.
fn command_name(text: &str) -> Option<String> {
    let start = text.find("<command-name>")? + "<command-name>".len();
    let len = text[start..].find("</command-name>")?;
    let name = text[start..start + len].trim().trim_start_matches('/');
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_all(text: &str) -> Vec<SkillEvent> {
        let mut p = Parser::default();
        text.lines().flat_map(|l| p.feed(l)).collect()
    }

    fn names(events: &[SkillEvent]) -> Vec<(&str, &Via)> {
        events.iter().map(|e| (e.name.as_str(), &e.via)).collect()
    }

    #[test]
    fn fixture_transcript() {
        let events = parse_all(include_str!("../tests/fixtures/transcript.jsonl"));
        assert_eq!(
            names(&events),
            vec![
                ("superpowers:brainstorming", &Via::Tool),
                ("simplify", &Via::Slash),
                ("writing-plans", &Via::Slash),
                ("superpowers:test-driven-development", &Via::Tool),
                ("simplify", &Via::Tool),
            ]
        );
        // A slash skill is timed by the command entry, not the meta entry.
        assert_eq!(
            events[1].timestamp.to_rfc3339(),
            "2026-10-14T21:30:00+00:00"
        );
    }

    #[test]
    fn duplicate_tool_use_id_emits_once() {
        let line = r#"{"type":"assistant","timestamp":"2026-10-14T21:00:00Z","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Skill","input":{"skill":"a"}}]}}"#;
        let mut p = Parser::default();
        assert_eq!(p.feed(line).len(), 1);
        assert_eq!(p.feed(line).len(), 0);
    }

    #[test]
    fn command_name_extraction() {
        assert_eq!(
            command_name("<command-name>/simplify</command-name>"),
            Some("simplify".into())
        );
        assert_eq!(
            command_name("<command-name>review</command-name>"),
            Some("review".into())
        );
        assert_eq!(command_name("<command-name></command-name>"), None);
        assert_eq!(command_name("<command-name>/x"), None);
        assert_eq!(command_name("no command"), None);
    }
}
