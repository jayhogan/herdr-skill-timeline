//! Turns Claude Code transcript lines into skill events.
//!
//! Two ways a skill shows up in a transcript:
//! - Claude calls it: an `assistant` entry with a `tool_use` block named `Skill`.
//! - The user types `/name`: a `user` entry containing
//!   `<command-name>/name</command-name>`, confirmed by a later `isMeta` user
//!   entry starting with "Base directory for this skill:". Built-in commands
//!   (`/clear`, `/model`, ...) never get that confirmation.
//! - The user types a plugin command that runs locally (`/plannotator-review`):
//!   a `system` entry whose `commandRun` names the plugin.
//!
//! It also turns pull requests into events: a `Bash` call running
//! `gh pr create` or `gh pr merge` whose result is not an error becomes
//! "PR 1234 created" or "PR 1234 merged", timed by the result.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Via {
    Tool,
    Slash,
}

/// One timeline entry: a skill run, or a PR event (`name` is then the
/// display text, e.g. "PR 1234 merged").
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
    /// Commands of `gh pr` Bash calls waiting for their result, by `tool_use` id.
    pr_calls: HashMap<String, String>,
    /// PR events already emitted, so a retried create shows once.
    seen_prs: HashSet<(PrAction, u64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PrAction {
    Created,
    Merged,
}

impl PrAction {
    fn of(command: &str) -> Option<(Self, Option<u64>)> {
        if command.contains("gh pr create") {
            Some((Self::Created, None))
        } else {
            let args = command.split("gh pr merge").nth(1)?;
            Some((Self::Merged, merge_arg(args)))
        }
    }
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
                self.note_pr_calls(content);
                self.skill_tool_uses(content, timestamp)
            }
            Some("user") => {
                let prs = self.pr_results(content, timestamp);
                if !prs.is_empty() {
                    return prs;
                }
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
            // Plugin commands run locally: the result entry's `commandRun`
            // names the plugin. Built-ins like `/rename` have no plugin.
            Some("system") => {
                let Some(run) = entry.get("commandRun") else {
                    return vec![];
                };
                match (
                    run.get("plugin").and_then(Value::as_str),
                    run.get("command").and_then(Value::as_str),
                ) {
                    (Some(_), Some(name)) if !name.is_empty() => vec![SkillEvent {
                        name: name.to_string(),
                        timestamp,
                        via: Via::Slash,
                    }],
                    _ => vec![],
                }
            }
            _ => vec![],
        }
    }

    fn note_pr_calls(&mut self, content: Option<&Value>) {
        for block in content.and_then(Value::as_array).into_iter().flatten() {
            if block.get("type").and_then(Value::as_str) != Some("tool_use")
                || block.get("name").and_then(Value::as_str) != Some("Bash")
            {
                continue;
            }
            let (Some(id), Some(command)) = (
                block.get("id").and_then(Value::as_str),
                block.pointer("/input/command").and_then(Value::as_str),
            ) else {
                continue;
            };
            if PrAction::of(command).is_some() {
                self.pr_calls.insert(id.to_string(), command.to_string());
            }
        }
    }

    /// PR events for the successful results of pending `gh pr` calls.
    fn pr_results(&mut self, content: Option<&Value>, ts: DateTime<Utc>) -> Vec<SkillEvent> {
        let mut events = vec![];
        for block in content.and_then(Value::as_array).into_iter().flatten() {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(command) = block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .and_then(|id| self.pr_calls.remove(id))
            else {
                continue;
            };
            if block.get("is_error").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let Some((action, from_command)) = PrAction::of(&command) else {
                continue;
            };
            let output = text_of(block.get("content"));
            if action == PrAction::Merged && merge_failed(&output) {
                continue;
            }
            let number = match action {
                PrAction::Created => pull_number(&output),
                PrAction::Merged => from_command.or_else(|| merged_number(&output)),
            };
            let Some(number) = number else { continue };
            if !self.seen_prs.insert((action, number)) {
                continue;
            }
            let verb = match action {
                PrAction::Created => "created",
                PrAction::Merged => "merged",
            };
            events.push(SkillEvent {
                name: format!("PR {number} {verb}"),
                timestamp: ts,
                via: Via::Tool,
            });
        }
        events
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

/// The PR named by `gh pr merge`'s arguments: the first positional number or
/// `/pull/N` URL before the end of that command.
fn merge_arg(args: &str) -> Option<u64> {
    let end = args.find(['|', ';', '&', '\n']).unwrap_or(args.len());
    args[..end]
        .split_whitespace()
        .filter(|a| !a.starts_with('-'))
        .find_map(|a| a.parse().ok().or_else(|| pull_number(a)))
}

/// `N` from the first `/pull/N` in `text` (skipping `git push`'s
/// `/pull/new/<branch>` hint).
fn pull_number(text: &str) -> Option<u64> {
    text.split("/pull/").skip(1).find_map(leading_number)
}

/// `N` from "merged pull request owner/repo#N" in `gh pr merge` output.
fn merged_number(text: &str) -> Option<u64> {
    let rest = &text[text.find("pull request")?..];
    leading_number(&rest[rest.find('#')? + 1..])
}

/// Whether `gh pr merge` output shows the merge didn't happen. A pipeline
/// like `gh pr merge 4 | tail -3; gh pr view 4` succeeds even when the merge
/// fails, so the error text (or a following `gh pr view`'s `OPEN` state) is
/// the only sign.
fn merge_failed(output: &str) -> bool {
    let lower = output.to_lowercase();
    [
        "graphql:",
        "not mergeable",
        "not allowed",
        "failed",
        "could not",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
        || output.split_whitespace().any(|word| word == "OPEN")
}

fn leading_number(s: &str) -> Option<u64> {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
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
    fn plugin_local_command_is_a_skill() {
        // Shapes copied from a real transcript: `/plannotator-review` runs
        // as a plugin local command, with no `Skill` call or skill meta entry.
        let text = r#"{"type":"system","subtype":"local_command","content":"<command-name>/plannotator-review</command-name>","timestamp":"2026-10-16T12:32:26.511Z","isMeta":false}
{"type":"system","subtype":"local_command","content":"<local-command-stdout>plannotator: Opened local changes</local-command-stdout>","timestamp":"2026-10-16T12:32:26.511Z","isMeta":false,"commandRun":{"command":"plannotator-review","args":"","plugin":"plannotator"}}
{"type":"system","subtype":"local_command","content":"<command-name>/rename</command-name>","timestamp":"2026-10-16T12:33:00.000Z","isMeta":false}
{"type":"system","subtype":"local_command","content":"<local-command-stdout>Session renamed to: main</local-command-stdout>","timestamp":"2026-10-16T12:33:00.000Z","isMeta":false,"commandRun":{"command":"rename","args":"main"}}"#;
        let events = parse_all(text);
        assert_eq!(names(&events), vec![("plannotator-review", &Via::Slash)]);
        assert_eq!(
            events[0].timestamp.to_rfc3339(),
            "2026-10-16T12:32:26.511+00:00"
        );
    }

    /// An assistant `Bash` call followed by its result, as transcript lines.
    fn bash(id: &str, command: &str, output: &str, is_error: bool, at: &str) -> String {
        let call = serde_json::json!({
            "type": "assistant", "timestamp": at,
            "message": {"content": [{"type": "tool_use", "id": id, "name": "Bash",
                "input": {"command": command}}]}
        });
        let result = serde_json::json!({
            "type": "user", "timestamp": at,
            "message": {"content": [{"type": "tool_result", "tool_use_id": id,
                "is_error": is_error, "content": output}]}
        });
        format!("{call}\n{result}")
    }

    #[test]
    fn pr_created_from_chained_push_and_create() {
        let text = bash(
            "toolu_c",
            "git push -q -u origin ci-red-bug 2>&1 | tail -2\ngh pr create --base main --title \"Fix\" --body \"x\"",
            "remote:      https://github.com/jayhogan/herdr-pr-pulse/pull/new/ci-red-bug\nremote: \nhttps://github.com/jayhogan/herdr-pr-pulse/pull/4",
            false,
            "2026-10-16T12:00:00Z",
        );
        let events = parse_all(&text);
        assert_eq!(names(&events), vec![("PR 4 created", &Via::Tool)]);
        assert_eq!(
            events[0].timestamp.to_rfc3339(),
            "2026-10-16T12:00:00+00:00"
        );
    }

    #[test]
    fn pr_merged_number_from_command() {
        let text = bash(
            "toolu_m",
            "gh pr merge 2229 --squash --match-head-commit 8442ae5f 2>&1 | tail -3; gh pr view 2229 --json state",
            "MERGED 2026-11-05T16:59:04Z 817b8231ce",
            false,
            "2026-11-05T17:00:00Z",
        );
        assert_eq!(
            names(&parse_all(&text)),
            vec![("PR 2229 merged", &Via::Tool)]
        );
    }

    #[test]
    fn pr_merged_number_from_output_when_command_has_none() {
        let text = bash(
            "toolu_m",
            "gh pr merge --squash --delete-branch",
            "✓ Squashed and merged pull request jayhogan/herdr-skill-timeline#3 (fix)\n✓ Deleted local branch",
            false,
            "2026-11-05T17:00:00Z",
        );
        assert_eq!(names(&parse_all(&text)), vec![("PR 3 merged", &Via::Tool)]);
    }

    #[test]
    fn failed_or_unrelated_commands_are_not_pr_events() {
        let text = [
            bash(
                "t1",
                "gh pr merge 7 --squash",
                "Permission denied",
                true,
                "2026-11-05T17:00:00Z",
            ),
            bash(
                "t2",
                "gh pr view 7",
                "https://github.com/o/r/pull/7",
                false,
                "2026-11-05T17:01:00Z",
            ),
            bash(
                "t3",
                "gh pr create --fill",
                "a pull request already exists",
                false,
                "2026-11-05T17:02:00Z",
            ),
        ]
        .join("\n");
        assert!(parse_all(&text).is_empty());
    }

    #[test]
    fn failed_merge_in_a_successful_pipeline_is_not_a_merge() {
        // From a real session: the merge failed, but `| tail -3; gh pr view`
        // made the whole command succeed. The squash retry is the merge.
        let text = [
            bash(
                "t1",
                "gh pr merge 4 --merge 2>&1 | tail -3; gh pr view 4 --json state,mergedAt -q '\"\\(.state) \\(.mergedAt)\"'",
                "GraphQL: Merge commits are not allowed on this repository. (mergePullRequest)\nOPEN null",
                false,
                "2026-11-12T11:54:00Z",
            ),
            bash(
                "t2",
                "gh pr merge 4 --squash 2>&1 | tail -3; gh pr view 4 --json state,mergedAt -q '\"\\(.state) \\(.mergedAt)\"'",
                "MERGED 2026-11-12T11:54:36Z",
                false,
                "2026-11-12T11:54:37Z",
            ),
        ]
        .join("\n");
        let events = parse_all(&text);
        assert_eq!(names(&events), vec![("PR 4 merged", &Via::Tool)]);
        assert_eq!(
            events[0].timestamp.to_rfc3339(),
            "2026-11-12T11:54:37+00:00"
        );
    }

    #[test]
    fn merge_error_messages_are_not_merges() {
        for output in [
            "X Pull request jayhogan/r#7 is not mergeable: the merge commit cannot be cleanly created.",
            "failed to run git: fatal: not a git repository",
            "could not determine current branch",
            "OPEN",
        ] {
            let text = bash(
                "t",
                "gh pr merge 7 --squash | tail -3",
                output,
                false,
                "2026-11-12T12:00:00Z",
            );
            assert!(parse_all(&text).is_empty(), "counted as merged: {output}");
        }
    }

    #[test]
    fn same_pr_created_twice_shows_once() {
        let text = [
            bash(
                "t1",
                "gh pr create --fill",
                "https://github.com/o/r/pull/1857",
                false,
                "2026-10-21T13:00:00Z",
            ),
            bash(
                "t2",
                "cd x && gh pr create --fill",
                "https://github.com/o/r/pull/1857",
                false,
                "2026-10-21T13:01:00Z",
            ),
            bash(
                "t3",
                "gh pr merge 1857 --squash",
                "",
                false,
                "2026-10-21T13:15:00Z",
            ),
        ]
        .join("\n");
        assert_eq!(
            names(&parse_all(&text)),
            vec![
                ("PR 1857 created", &Via::Tool),
                ("PR 1857 merged", &Via::Tool)
            ]
        );
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
