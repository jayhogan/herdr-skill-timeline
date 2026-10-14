# herdr-skill-timeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust TUI, run in any herdr pane, that shows the Claude skills each Claude session in the current workspace has run, grouped by pane, newest first, with "10 min ago" labels. It also includes a herdr event hook that remembers each pane's sessions across `/clear`.

**Architecture:** One binary with two modes. With no arguments it runs the TUI. Every 2s the TUI reads `herdr api snapshot`, records each Claude pane's session in a locked `sessions.json`, reads only the new lines of each session's transcript (`~/.claude/projects/*/<id>.jsonl`), parses out skill invocations, and redraws. `record` is the herdr event hook. It saves a pane's current session so history survives `/clear` even while the TUI is closed. Pure logic (parsing, labels, grouping, rendering) is separated from IO, so almost everything is unit-tested.

**Tech Stack:** Rust (edition 2024, rustc ≥ 1.89), ratatui 0.30, crossterm 0.29, serde/serde_json, chrono, tempfile (tests only).

**Spec:** `docs/superpowers/specs/2026-10-14-skill-timeline-design.md`

**Note for the implementer:** every code block below was compiled and tested as a whole crate before the plan was written: 33 tests passing, no clippy warnings, rustfmt-clean. Copy it exactly. If something fails, stop and report it instead of improvising.

## Global Constraints

- Repository: `~/dev/herdr-skill-timeline`, crate and binary name `herdr-skill-timeline`.
- Rust edition 2024, rustc ≥ 1.89 (uses std `File::lock` and `let` chains). Toolchain here is 1.99.
- Platforms: macOS and Linux only (`platforms = ["linux", "macos"]`).
- Dependencies exactly as in Task 1's `Cargo.toml`. No other crates.
- State file: `${XDG_STATE_HOME:-~/.local/state}/herdr-skill-timeline/sessions.json`, never `HERDR_PLUGIN_STATE_DIR`.
- Transcripts: `${CLAUDE_CONFIG_DIR:-~/.claude}/projects/*/<session_id>.jsonl`, read-only.
- `record` mode always exits 0 and never writes to stdout.
- TUI outside herdr: exactly `herdr-skill-timeline: not running inside a herdr pane` on stderr, exit 1.
- Refresh interval: 2 seconds.
- Relative labels: `just now` / `N min ago` / `N hr ago` / `yesterday` / `N days ago`.

## Review Focus

These are the most likely real-world failures. Each is pinned by a test in the task that owns the code:

1. **herdr restarting or returning a snapshot without this pane must not wipe history.** Pinned by `timeline::tests::unreachable_keeps_last_view_and_registry` (Task 6).
2. **`api-request-blob` entries repeat old `Skill` tool_use blocks and must not count.** Pinned by the fixture line in `parse::tests::fixture_transcript` (Task 2).
3. **Resumed or forked sessions repeat history, so a skill must not show twice.** Pinned by `parse::tests::duplicate_tool_use_id_emits_once` (Task 2).
4. **The event hook fires on every agent status change in every pane. Non-Claude panes must not touch the state file.** Pinned by `record::tests::ignores_non_claude_and_sessionless_panes_without_writing` (Task 7).
5. **Very narrow panes must truncate names, keep times and never panic.** Pinned by `ui::tests::narrow_pane_truncates_names_not_times` and `tiny_pane_does_not_panic` (Task 8).

## File Structure

| File | Responsibility |
|---|---|
| `Cargo.toml`, `.gitignore` | Crate definition |
| `herdr-plugin.toml` | Plugin manifest: build step + two event hooks |
| `scripts/install.sh` | Symlink the binary into `~/.local/bin` |
| `README.md` | Install and usage |
| `src/main.rs` | Mode dispatch, TUI event loop |
| `src/ago.rs` | `label(ts, now)` relative time |
| `src/parse.rs` | `Parser::feed(line) -> Vec<SkillEvent>` |
| `src/transcript.rs` | `projects_dir()`, `find_transcript()`, `Tail::read_new_lines()` |
| `src/herdr.rs` | `Snapshot`, `fetch_snapshot()`, `parse_snapshot()`, `ClaudePane` |
| `src/registry.rs` | `Registry` (pane → sessions), `Store::update()` with file lock |
| `src/timeline.rs` | `App::refresh()` → `View`; `Feeds`; grouping and ordering |
| `src/record.rs` | `record` mode |
| `src/ui.rs` | `render()` / `lines()` with ratatui |
| `tests/fixtures/transcript.jsonl`, `tests/fixtures/snapshot.json` | Test data |

Tests are Rust unit tests in a `#[cfg(test)] mod tests` at the bottom of each source file, the usual layout for a binary crate. Fixtures are pulled in with `include_str!`.

**TDD pattern used in every task.** Step 1 creates the source file containing only its test module. That fails to compile, which is the expected "red" for Rust. A later step adds the implementation above the test module. Intermediate tasks may print `dead_code` warnings for items that later tasks use. That is expected; only Task 9 must be warning-free.

---

### Task 1: Crate scaffold and relative time labels

**Files:**
- Create: `Cargo.toml`, `.gitignore`, `src/main.rs`, `src/ago.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `ago::label(ts: chrono::DateTime<Utc>, now: DateTime<Utc>) -> String`.

- [ ] **Step 1: Create the crate files**

`Cargo.toml`:
```toml
[package]
name = "herdr-skill-timeline"
version = "0.1.0"
edition = "2024"

[dependencies]
chrono = { version = "0.4", default-features = false, features = ["std", "clock", "serde"] }
crossterm = "0.29"
ratatui = "0.30"
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[dev-dependencies]
tempfile = "3"
```

`.gitignore`:
```
/target
```

`src/main.rs` (temporary; grows each task, final version in Task 9):
```rust
mod ago;

fn main() {}
```

- [ ] **Step 2: Write the failing tests** in `src/ago.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn at(secs_ago: i64) -> String {
        let now = DateTime::parse_from_rfc3339("2026-10-14T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        label(now - Duration::seconds(secs_ago), now)
    }

    #[test]
    fn boundaries() {
        assert_eq!(at(0), "just now");
        assert_eq!(at(59), "just now");
        assert_eq!(at(60), "1 min ago");
        assert_eq!(at(59 * 60 + 59), "59 min ago");
        assert_eq!(at(60 * 60), "1 hr ago");
        assert_eq!(at(23 * 3600 + 3599), "23 hr ago");
        assert_eq!(at(24 * 3600), "yesterday");
        assert_eq!(at(47 * 3600 + 3599), "yesterday");
        assert_eq!(at(48 * 3600), "2 days ago");
        assert_eq!(at(10 * 24 * 3600), "10 days ago");
    }

    #[test]
    fn future_is_just_now() {
        assert_eq!(at(-300), "just now");
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test ago::`
Expected: compile error `cannot find function `label` in this scope`.

- [ ] **Step 4: Add the implementation** at the top of `src/ago.rs`, above `#[cfg(test)]`:

```rust
//! Relative "10 min ago" labels.

use chrono::{DateTime, Utc};

/// Label for how long ago `ts` was, relative to `now`.
/// Future timestamps (clock skew) read as "just now".
pub fn label(ts: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - ts).num_seconds();
    if secs < 60 {
        return "just now".to_string();
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins} min ago");
    }
    let hrs = mins / 60;
    if hrs < 24 {
        return format!("{hrs} hr ago");
    }
    if hrs < 48 {
        return "yesterday".to_string();
    }
    format!("{} days ago", hrs / 24)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test ago::`
Expected: `test result: ok. 2 passed`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock .gitignore src/main.rs src/ago.rs
git commit -m "feat: crate scaffold and relative time labels"
```

---

### Task 2: Transcript line parser

**Files:**
- Create: `src/parse.rs`, `tests/fixtures/transcript.jsonl`
- Modify: `src/main.rs` (add `mod parse;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `parse::Via { Tool, Slash }` (derives `Debug, Clone, PartialEq, Eq`).
  - `parse::SkillEvent { pub name: String, pub timestamp: DateTime<Utc>, pub via: Via }` (derives `Debug, Clone, PartialEq, Eq`).
  - `parse::Parser` (derives `Debug, Default`), with `fn feed(&mut self, line: &str) -> Vec<SkillEvent>`. Construct it with `Parser::default()`.

The fixture covers, in order:
- a `Skill` tool_use followed by its own "Base directory" meta and tool_result, which must emit once;
- an `api-request-blob` repeating an old tool_use, which must not count;
- `/simplify` typed and confirmed;
- `/clear`;
- `/model` followed by a hook-output meta, then an assistant reply, then a stray "Base directory" meta, which must not emit;
- a sidechain Skill;
- a truncated JSON line;
- `/writing-plans` with an unrelated meta entry before its confirmation, which must emit;
- one assistant entry with two Skill calls and a Bash call.

- [ ] **Step 1: Create the fixture** `tests/fixtures/transcript.jsonl` (each line is one JSON object; line 15 is deliberately truncated):

```
{"type":"user","isSidechain":false,"timestamp":"2026-10-14T21:20:00.000Z","message":{"role":"user","content":"Help me plan a plugin"}}
{"type":"assistant","isSidechain":false,"timestamp":"2026-10-14T21:23:01.455Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_A","name":"Skill","input":{"skill":"superpowers:brainstorming"}}]}}
{"type":"user","isSidechain":false,"isMeta":true,"timestamp":"2026-10-14T21:23:01.481Z","message":{"role":"user","content":[{"type":"text","text":"Base directory for this skill: /x/brainstorming\n\n# Brainstorming"}]}}
{"type":"user","isSidechain":false,"timestamp":"2026-10-14T21:23:01.500Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_A","content":"Launching skill: superpowers:brainstorming"}]}}
{"type":"api-request-blob","message":{"content":[{"type":"tool_use","id":"toolu_OLD","name":"Skill","input":{"skill":"must-not-count"}}]}}
{"type":"user","isSidechain":false,"timestamp":"2026-10-14T21:30:00.000Z","message":{"role":"user","content":"<command-message>simplify</command-message>\n<command-name>/simplify</command-name>"}}
{"type":"user","isSidechain":false,"isMeta":true,"timestamp":"2026-10-14T21:30:00.050Z","message":{"role":"user","content":[{"type":"text","text":"Base directory for this skill: /x/simplify\n\nAudit the diff"}]}}
{"type":"user","isSidechain":false,"timestamp":"2026-10-14T21:35:00.000Z","message":{"role":"user","content":"<command-name>/clear</command-name>\n<command-message>clear</command-message>\n<command-args></command-args>"}}
{"type":"system","timestamp":"2026-10-14T21:35:00.010Z"}
{"type":"user","isSidechain":false,"timestamp":"2026-10-14T21:36:00.000Z","message":{"role":"user","content":"<command-name>/model</command-name>"}}
{"type":"user","isSidechain":false,"isMeta":true,"timestamp":"2026-10-14T21:36:00.010Z","message":{"role":"user","content":[{"type":"text","text":"Hook output: model set"}]}}
{"type":"assistant","isSidechain":false,"timestamp":"2026-10-14T21:36:05.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Model changed."}]}}
{"type":"user","isSidechain":false,"isMeta":true,"timestamp":"2026-10-14T21:36:06.000Z","message":{"role":"user","content":[{"type":"text","text":"Base directory for this skill: /x/stray"}]}}
{"type":"assistant","isSidechain":true,"timestamp":"2026-10-14T21:37:00.000Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_SIDE","name":"Skill","input":{"skill":"subagent-only"}}]}}
{"type":"assistant","isSidechain":false,"timestamp":"2026-10-14T21:38
{"type":"user","isSidechain":false,"timestamp":"2026-10-14T21:40:00.000Z","message":{"role":"user","content":"<command-message>writing-plans</command-message>\n<command-name>/writing-plans</command-name>"}}
{"type":"user","isSidechain":false,"isMeta":true,"timestamp":"2026-10-14T21:40:00.010Z","message":{"role":"user","content":[{"type":"text","text":"SessionStart hook additional context"}]}}
{"type":"user","isSidechain":false,"isMeta":true,"timestamp":"2026-10-14T21:40:00.020Z","message":{"role":"user","content":[{"type":"text","text":"Base directory for this skill: /x/writing-plans"}]}}
{"type":"assistant","isSidechain":false,"timestamp":"2026-10-14T21:50:00.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Starting with tests."},{"type":"tool_use","id":"toolu_B","name":"Skill","input":{"skill":"superpowers:test-driven-development"}},{"type":"tool_use","id":"toolu_C","name":"Skill","input":{"skill":"simplify"}},{"type":"tool_use","id":"toolu_D","name":"Bash","input":{"command":"ls"}}]}}
```

- [ ] **Step 2: Write the failing tests.** Create `src/parse.rs`:

```rust
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
```

Add `mod parse;` to `src/main.rs` (keep modules in alphabetical order).

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test parse::`
Expected: compile errors (`cannot find type `SkillEvent``, `Parser`, `command_name`).

- [ ] **Step 4: Add the implementation** at the top of `src/parse.rs`:

```rust
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
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test parse::`
Expected: `test result: ok. 3 passed`.

- [ ] **Step 6: Commit**

```bash
git add src/parse.rs src/main.rs tests/fixtures/transcript.jsonl
git commit -m "feat: parse skill invocations from Claude transcripts"
```

---

### Task 3: Transcript location and incremental reading

**Files:**
- Create: `src/transcript.rs`
- Modify: `src/main.rs` (add `mod transcript;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `transcript::projects_dir() -> PathBuf`.
  - `transcript::find_transcript(projects_dir: &Path, session_id: &str) -> Option<PathBuf>`.
  - `transcript::Tail::new(path: PathBuf) -> Tail`.
  - `Tail::read_new_lines(&mut self) -> io::Result<Vec<String>>`, which returns only complete lines added since the previous call. A partial last line waits for the next call.

- [ ] **Step 1: Write the failing tests.** Create `src/transcript.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn finds_transcript_in_any_project_dir() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("-Users-a-repo")).unwrap();
        fs::create_dir_all(dir.path().join("-Users-a-repo-worktree")).unwrap();
        let target = dir.path().join("-Users-a-repo-worktree").join("abc.jsonl");
        fs::write(&target, "").unwrap();
        assert_eq!(find_transcript(dir.path(), "abc"), Some(target));
        assert_eq!(find_transcript(dir.path(), "missing"), None);
        assert_eq!(find_transcript(&dir.path().join("nope"), "abc"), None);
    }

    #[test]
    fn reads_incrementally_and_holds_partial_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        fs::write(&path, "one\ntwo\nthr").unwrap();
        let mut tail = Tail::new(path.clone());
        assert_eq!(tail.read_new_lines().unwrap(), vec!["one", "two"]);
        assert!(tail.read_new_lines().unwrap().is_empty());

        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(f, "ee\nfour\n").unwrap();
        assert_eq!(tail.read_new_lines().unwrap(), vec!["three", "four"]);
        assert!(tail.read_new_lines().unwrap().is_empty());
    }

    #[test]
    fn missing_file_is_an_error_not_a_panic() {
        let mut tail = Tail::new(PathBuf::from("/definitely/not/here.jsonl"));
        assert!(tail.read_new_lines().is_err());
    }
}
```

Add `mod transcript;` to `src/main.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test transcript::`
Expected: compile errors (`cannot find function `find_transcript``, `Tail`).

- [ ] **Step 3: Add the implementation** at the top of `src/transcript.rs`:

```rust
//! Locating Claude transcripts and reading only what was appended since the
//! last read.

use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// `${CLAUDE_CONFIG_DIR:-~/.claude}/projects`.
pub fn projects_dir() -> PathBuf {
    let base = env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".claude"));
    base.join("projects")
}

/// Finds `<projects_dir>/*/<session_id>.jsonl`. Any project directory may
/// hold it (Claude names them after the session's starting cwd).
pub fn find_transcript(projects_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let file_name = format!("{session_id}.jsonl");
    fs::read_dir(projects_dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(&file_name))
        .find(|path| path.is_file())
}

/// Incremental reader for one append-only file.
#[derive(Debug)]
pub struct Tail {
    path: PathBuf,
    offset: u64,
}

impl Tail {
    pub fn new(path: PathBuf) -> Self {
        Self { path, offset: 0 }
    }

    /// Complete lines appended since the last call. A trailing line without
    /// a newline (Claude mid-write) is left for the next call.
    pub fn read_new_lines(&mut self) -> io::Result<Vec<String>> {
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(self.offset))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        let Some(last_newline) = buf.iter().rposition(|&b| b == b'\n') else {
            return Ok(vec![]);
        };
        let complete = &buf[..=last_newline];
        self.offset += complete.len() as u64;
        Ok(String::from_utf8_lossy(complete)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_owned)
            .collect())
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test transcript::`
Expected: `test result: ok. 3 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/transcript.rs src/main.rs
git commit -m "feat: locate transcripts and read appended lines"
```

---

### Task 4: herdr snapshot

**Files:**
- Create: `src/herdr.rs`, `tests/fixtures/snapshot.json`
- Modify: `src/main.rs` (add `mod herdr;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `herdr::Snapshot { pub workspaces, pub panes, pub agents }` (`Debug, Clone, Default, Deserialize`). Each field defaults to empty.
  - `herdr::Agent { pub pane_id, pub workspace_id, pub agent: String, pub terminal_title_stripped: Option<String>, pub agent_session: Option<AgentSession> }`.
  - `herdr::AgentSession { pub kind: String, pub value: String }`.
  - `herdr::ClaudePane { pub pane_id: String, pub workspace_id: String, pub session_id: Option<String>, pub title: Option<String> }` (`Debug, Clone, PartialEq, Eq`).
  - `herdr::parse_snapshot(json: &str) -> Result<Snapshot, String>`.
  - `herdr::fetch_snapshot() -> Result<Snapshot, String>`, which runs `$HERDR_BIN_PATH api snapshot` and falls back to `herdr`.
  - Methods on `Snapshot`:
    - `workspace_of(&self, pane_id) -> Option<&str>`
    - `workspace_label(&self, ws) -> Option<&str>`
    - `live_pane_ids(&self) -> BTreeSet<String>`
    - `claude_panes(&self) -> Vec<ClaudePane>`
    - `claude_panes_in(&self, ws) -> Vec<ClaudePane>`

The real `herdr api snapshot` output looks like `{"id":…,"type":"snapshot","result":{"snapshot":{…}}}`. A Claude session ID is `agents[].agent_session.value` when `kind == "id"`.

- [ ] **Step 1: Create the fixture** `tests/fixtures/snapshot.json`. Workspace `w1` has these panes:
  - `p1`: Claude with session `sess-1`
  - `p2`: Claude, no session reported yet
  - `p3`: a plain shell, where the TUI itself will run
  - `p4`: Codex
  - `p5`: Claude with a `path`-kind session, which must be ignored

  Workspace `w2` has a Claude pane `p1`.

```json
{
  "id": "cli:api:snapshot",
  "type": "snapshot",
  "result": {
    "snapshot": {
      "protocol": 22,
      "version": "0.9.3",
      "focused_pane_id": "w1:p1",
      "workspaces": [
        { "workspace_id": "w1", "label": "core-8920-store-blockers", "number": 1 },
        { "workspace_id": "w2", "label": "docket", "number": 2 }
      ],
      "panes": [
        { "pane_id": "w1:p1", "workspace_id": "w1", "tab_id": "w1:t1", "agent_status": "working" },
        { "pane_id": "w1:p2", "workspace_id": "w1", "tab_id": "w1:t1", "agent_status": "idle" },
        { "pane_id": "w1:p3", "workspace_id": "w1", "tab_id": "w1:t1" },
        { "pane_id": "w1:p4", "workspace_id": "w1", "tab_id": "w1:t1", "agent_status": "idle" },
        { "pane_id": "w1:p5", "workspace_id": "w1", "tab_id": "w1:t1", "agent_status": "idle" },
        { "pane_id": "w2:p1", "workspace_id": "w2", "tab_id": "w2:t1", "agent_status": "idle" }
      ],
      "agents": [
        {
          "pane_id": "w1:p1", "workspace_id": "w1", "agent": "claude", "agent_status": "working",
          "terminal_title_stripped": "store blockers",
          "agent_session": { "agent": "claude", "kind": "id", "source": "herdr:claude", "value": "sess-1" }
        },
        {
          "pane_id": "w1:p2", "workspace_id": "w1", "agent": "claude", "agent_status": "idle",
          "terminal_title_stripped": ""
        },
        {
          "pane_id": "w1:p4", "workspace_id": "w1", "agent": "codex", "agent_status": "idle",
          "agent_session": { "agent": "codex", "kind": "id", "source": "herdr:codex", "value": "codex-1" }
        },
        {
          "pane_id": "w1:p5", "workspace_id": "w1", "agent": "claude", "agent_status": "idle",
          "agent_session": { "agent": "claude", "kind": "path", "source": "herdr:claude", "value": "/tmp/x.jsonl" }
        },
        {
          "pane_id": "w2:p1", "workspace_id": "w2", "agent": "claude", "agent_status": "idle",
          "agent_session": { "agent": "claude", "kind": "id", "source": "herdr:claude", "value": "sess-9" }
        }
      ]
    }
  }
}
```

- [ ] **Step 2: Write the failing tests.** Create `src/herdr.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Snapshot {
        parse_snapshot(include_str!("../tests/fixtures/snapshot.json")).unwrap()
    }

    #[test]
    fn workspace_lookup() {
        let s = fixture();
        assert_eq!(s.workspace_of("w1:p3"), Some("w1"));
        assert_eq!(s.workspace_of("nope"), None);
        assert_eq!(s.workspace_label("w1"), Some("core-8920-store-blockers"));
        assert_eq!(s.live_pane_ids().len(), 6);
    }

    #[test]
    fn claude_panes_in_workspace() {
        let panes = fixture().claude_panes_in("w1");
        assert_eq!(
            panes,
            vec![
                ClaudePane {
                    pane_id: "w1:p1".into(),
                    workspace_id: "w1".into(),
                    session_id: Some("sess-1".into()),
                    title: Some("store blockers".into()),
                },
                // Claude detected but session not reported yet.
                ClaudePane {
                    pane_id: "w1:p2".into(),
                    workspace_id: "w1".into(),
                    session_id: None,
                    title: None,
                },
                // Session reported as a path rather than an id: ignored.
                ClaudePane {
                    pane_id: "w1:p5".into(),
                    workspace_id: "w1".into(),
                    session_id: None,
                    title: None,
                },
            ]
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_snapshot("not json").is_err());
        assert!(parse_snapshot(r#"{"result":{}}"#).is_err());
    }

    #[test]
    fn tolerates_missing_lists() {
        let s = parse_snapshot(r#"{"result":{"snapshot":{}}}"#).unwrap();
        assert!(s.claude_panes().is_empty());
    }
}
```

Add `mod herdr;` to `src/main.rs`.

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test herdr::`
Expected: compile errors (`cannot find function `parse_snapshot``, `Snapshot`, `ClaudePane`).

- [ ] **Step 4: Add the implementation** at the top of `src/herdr.rs`:

```rust
//! Reading herdr's live state via `herdr api snapshot`.

use std::collections::BTreeSet;
use std::env;
use std::process::Command;

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub panes: Vec<Pane>,
    #[serde(default)]
    pub agents: Vec<Agent>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Workspace {
    pub workspace_id: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Pane {
    pub pane_id: String,
    pub workspace_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Agent {
    pub pane_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub terminal_title_stripped: Option<String>,
    #[serde(default)]
    pub agent_session: Option<AgentSession>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentSession {
    pub kind: String,
    pub value: String,
}

/// A pane running Claude, as the timeline needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudePane {
    pub pane_id: String,
    pub workspace_id: String,
    /// `None` until Claude's SessionStart hook has reported it.
    pub session_id: Option<String>,
    /// Terminal title, `None` when empty.
    pub title: Option<String>,
}

#[derive(Deserialize)]
struct Envelope {
    result: EnvelopeResult,
}

#[derive(Deserialize)]
struct EnvelopeResult {
    snapshot: Snapshot,
}

/// Parses the JSON printed by `herdr api snapshot`.
pub fn parse_snapshot(json: &str) -> Result<Snapshot, String> {
    serde_json::from_str::<Envelope>(json)
        .map(|e| e.result.snapshot)
        .map_err(|e| format!("invalid snapshot json: {e}"))
}

/// Runs `$HERDR_BIN_PATH api snapshot` (falls back to `herdr` on PATH).
pub fn fetch_snapshot() -> Result<Snapshot, String> {
    let bin = env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string());
    let output = Command::new(&bin)
        .args(["api", "snapshot"])
        .output()
        .map_err(|e| format!("could not run {bin}: {e}"))?;
    if !output.status.success() {
        return Err(format!("{bin} api snapshot exited with {}", output.status));
    }
    parse_snapshot(&String::from_utf8_lossy(&output.stdout))
}

impl Snapshot {
    pub fn workspace_of(&self, pane_id: &str) -> Option<&str> {
        self.panes
            .iter()
            .find(|p| p.pane_id == pane_id)
            .map(|p| p.workspace_id.as_str())
    }

    pub fn workspace_label(&self, workspace_id: &str) -> Option<&str> {
        self.workspaces
            .iter()
            .find(|w| w.workspace_id == workspace_id)
            .map(|w| w.label.as_str())
    }

    pub fn live_pane_ids(&self) -> BTreeSet<String> {
        self.panes.iter().map(|p| p.pane_id.clone()).collect()
    }

    /// Every pane running Claude, across all workspaces.
    pub fn claude_panes(&self) -> Vec<ClaudePane> {
        self.agents
            .iter()
            .filter(|a| a.agent == "claude")
            .map(|a| ClaudePane {
                pane_id: a.pane_id.clone(),
                workspace_id: a.workspace_id.clone(),
                session_id: a
                    .agent_session
                    .as_ref()
                    .filter(|s| s.kind == "id" && !s.value.is_empty())
                    .map(|s| s.value.clone()),
                title: a
                    .terminal_title_stripped
                    .as_ref()
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty()),
            })
            .collect()
    }

    pub fn claude_panes_in(&self, workspace_id: &str) -> Vec<ClaudePane> {
        self.claude_panes()
            .into_iter()
            .filter(|p| p.workspace_id == workspace_id)
            .collect()
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test herdr::`
Expected: `test result: ok. 4 passed`.

- [ ] **Step 6: Commit**

```bash
git add src/herdr.rs src/main.rs tests/fixtures/snapshot.json
git commit -m "feat: read herdr snapshot and find Claude panes"
```

---

### Task 5: Session registry with locked state file

**Files:**
- Create: `src/registry.rs`
- Modify: `src/main.rs` (add `mod registry;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `registry::SessionRecord { pub session_id: String, pub first_seen: DateTime<Utc> }`.
  - `registry::Registry { pub version: u32, pub panes: BTreeMap<String, Vec<SessionRecord>> }`, which derives `Default` with `version: 1`.
  - `Registry` methods:
    - `record(&mut self, pane_id: &str, session_id: &str, now) -> bool`, true if changed
    - `prune(&mut self, live: &BTreeSet<String>) -> bool`
    - `sessions_for(&self, pane_id) -> &[SessionRecord]`, oldest first
  - `registry::Store::new(dir: PathBuf)`.
  - `Store::default_location()`.
  - `Store::update(&self, change: impl FnOnce(&mut Registry) -> bool) -> io::Result<Registry>`. It takes the lock, loads the latest registry, applies `change`, and writes atomically if `change` returned true. Calling `update(|_| false)` is a locked read.

- [ ] **Step 1: Write the failing tests.** Create `src/registry.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn record_dedups_and_keeps_order() {
        let mut r = Registry::default();
        assert!(r.record("w1:p1", "a", t("2026-10-14T10:00:00Z")));
        assert!(!r.record("w1:p1", "a", t("2026-10-14T11:00:00Z")));
        assert!(r.record("w1:p1", "b", t("2026-10-14T12:00:00Z")));
        let ids: Vec<_> = r
            .sessions_for("w1:p1")
            .iter()
            .map(|s| s.session_id.as_str())
            .collect();
        assert_eq!(ids, vec!["a", "b"]);
        assert_eq!(
            r.sessions_for("w1:p1")[0].first_seen,
            t("2026-10-14T10:00:00Z")
        );
        assert!(r.sessions_for("other").is_empty());
    }

    #[test]
    fn prune_drops_dead_panes() {
        let mut r = Registry::default();
        r.record("w1:p1", "a", Utc::now());
        r.record("w1:p2", "b", Utc::now());
        let live = BTreeSet::from(["w1:p1".to_string()]);
        assert!(r.prune(&live));
        assert!(!r.prune(&live));
        assert_eq!(r.panes.keys().collect::<Vec<_>>(), vec!["w1:p1"]);
    }

    #[test]
    fn store_round_trip_and_sequential_writers() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("state"));
        store.update(|r| r.record("p1", "a", Utc::now())).unwrap();
        // A second writer (e.g. the event hook) sees the first writer's data.
        let other = Store::new(dir.path().join("state"));
        let r = other.update(|r| r.record("p2", "b", Utc::now())).unwrap();
        assert_eq!(r.panes.len(), 2);
        let reread = store.update(|_| false).unwrap();
        assert_eq!(reread, r);
    }

    #[test]
    fn concurrent_writers_do_not_lose_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    Store::new(path)
                        .update(|r| r.record(&format!("p{i}"), "s", Utc::now()))
                        .unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let r = Store::new(path).update(|_| false).unwrap();
        assert_eq!(r.panes.len(), 8);
    }

    #[test]
    fn corrupt_file_is_backed_up_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        fs::create_dir_all(&state).unwrap();
        fs::write(state.join("sessions.json"), "{not json").unwrap();
        let r = Store::new(state.clone())
            .update(|r| r.record("p1", "a", Utc::now()))
            .unwrap();
        assert_eq!(r.panes.len(), 1);
        assert_eq!(
            fs::read_to_string(state.join("sessions.json.bak")).unwrap(),
            "{not json"
        );
    }
}
```

Add `mod registry;` to `src/main.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test registry::`
Expected: compile errors (`cannot find type `Registry``, `Store`).

- [ ] **Step 3: Add the implementation** at the top of `src/registry.rs`:

```rust
//! Which Claude sessions each pane has had, persisted so history survives
//! `/clear` and restarts. Shared by the TUI and the `record` event hook.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File};
use std::io;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub first_seen: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    pub version: u32,
    /// pane id -> sessions, oldest first.
    pub panes: BTreeMap<String, Vec<SessionRecord>>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            version: 1,
            panes: BTreeMap::new(),
        }
    }
}

impl Registry {
    /// Adds `session_id` to the pane's history if new. Returns true if changed.
    pub fn record(&mut self, pane_id: &str, session_id: &str, now: DateTime<Utc>) -> bool {
        let sessions = self.panes.entry(pane_id.to_string()).or_default();
        if sessions.iter().any(|s| s.session_id == session_id) {
            return false;
        }
        sessions.push(SessionRecord {
            session_id: session_id.to_string(),
            first_seen: now,
        });
        true
    }

    /// Drops panes that no longer exist. Returns true if changed.
    pub fn prune(&mut self, live_pane_ids: &BTreeSet<String>) -> bool {
        let before = self.panes.len();
        self.panes
            .retain(|pane_id, _| live_pane_ids.contains(pane_id));
        self.panes.len() != before
    }

    pub fn sessions_for(&self, pane_id: &str) -> &[SessionRecord] {
        self.panes.get(pane_id).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// `sessions.json` on disk, guarded by an advisory lock on `sessions.json.lock`.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// `${XDG_STATE_HOME:-~/.local/state}/herdr-skill-timeline`.
    pub fn default_location() -> Self {
        let state_home = env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".local/state")
            });
        Self::new(state_home.join("herdr-skill-timeline"))
    }

    fn file(&self) -> PathBuf {
        self.dir.join("sessions.json")
    }

    /// Locks, loads the latest registry, applies `change`, writes it back if
    /// `change` returned true, and returns the resulting registry.
    pub fn update(&self, change: impl FnOnce(&mut Registry) -> bool) -> io::Result<Registry> {
        fs::create_dir_all(&self.dir)?;
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.dir.join("sessions.json.lock"))?;
        lock.lock()?;
        let mut registry = self.load_unlocked()?;
        if change(&mut registry) {
            let tmp = self.dir.join("sessions.json.tmp");
            fs::write(&tmp, serde_json::to_vec_pretty(&registry)?)?;
            fs::rename(&tmp, self.file())?;
        }
        Ok(registry)
        // `lock` is released when dropped here.
    }

    /// Missing file -> empty. Corrupt file -> moved to `sessions.json.bak`, empty.
    fn load_unlocked(&self) -> io::Result<Registry> {
        let bytes = match fs::read(self.file()) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Registry::default()),
            Err(e) => return Err(e),
        };
        match serde_json::from_slice(&bytes) {
            Ok(registry) => Ok(registry),
            Err(e) => {
                eprintln!("herdr-skill-timeline: sessions.json is corrupt ({e}); starting fresh");
                fs::rename(self.file(), self.dir.join("sessions.json.bak"))?;
                Ok(Registry::default())
            }
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test registry::`
Expected: `test result: ok. 5 passed` (`concurrent_writers_do_not_lose_records` runs 8 threads against one file).

- [ ] **Step 5: Commit**

```bash
git add src/registry.rs src/main.rs
git commit -m "feat: persist per-pane session history with file lock"
```

---

### Task 6: Refresh pass and view model

**Files:**
- Create: `src/timeline.rs`
- Modify: `src/main.rs` (add `mod timeline;`)

**Interfaces:**
- Consumes:
  - From Task 2: `Parser`, `SkillEvent`, `Via`.
  - From Task 3: `Tail`, `find_transcript`.
  - From Task 4: `Snapshot` and its methods, `AgentSession`, `parse_snapshot`.
  - From Task 5: `Registry`, `Store`.
- Produces:
  - `timeline::Status { Ok, Unreachable }`.
  - `timeline::Row { Skill { name: String, at: DateTime<Utc> }, NewSession }`.
  - `timeline::Group { pub pane_id: String, pub title: Option<String>, pub rows: Vec<Row> }`, with `latest()`. Its rows are newest first.
  - `timeline::View { pub title: String, pub status: Status, pub groups: Vec<Group> }`. All of these derive `Debug, Clone, PartialEq, Eq`.
  - `timeline::group_rows(&[Vec<SkillEvent>]) -> Vec<Row>` and `timeline::sort_groups(&mut [Group])`.
  - `timeline::Feeds::new(projects_dir: PathBuf)`.
  - `timeline::App::new(own_pane: String, store: Store, feeds: Feeds)`.
  - `App::refresh(&mut self, snapshot: Result<Snapshot, String>, now: DateTime<Utc>) -> View`.

- [ ] **Step 1: Write the failing tests.** Create `src/timeline.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::{AgentSession, parse_snapshot};
    use crate::parse::Via;
    use std::fs;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn ev(name: &str, at: &str) -> SkillEvent {
        SkillEvent {
            name: name.into(),
            timestamp: t(at),
            via: Via::Tool,
        }
    }

    fn skill(name: &str, at: &str) -> Row {
        Row::Skill {
            name: name.into(),
            at: t(at),
        }
    }

    #[test]
    fn rows_newest_first_with_session_dividers() {
        let sessions = vec![
            vec![
                ev("brainstorming", "2026-10-14T10:00:00Z"),
                ev("writing-plans", "2026-10-14T11:00:00Z"),
            ],
            vec![], // a session with no skills adds no divider
            vec![ev("tdd", "2026-10-14T12:00:00Z")],
        ];
        assert_eq!(
            group_rows(&sessions),
            vec![
                skill("tdd", "2026-10-14T12:00:00Z"),
                Row::NewSession,
                skill("writing-plans", "2026-10-14T11:00:00Z"),
                skill("brainstorming", "2026-10-14T10:00:00Z"),
            ]
        );
        assert!(group_rows(&[vec![], vec![]]).is_empty());
    }

    #[test]
    fn groups_sorted_by_latest_then_empty_by_id() {
        let g = |id: &str, rows: Vec<Row>| Group {
            pane_id: id.into(),
            title: None,
            rows,
        };
        let mut groups = vec![
            g("p3", vec![]),
            g("p1", vec![skill("a", "2026-10-14T10:00:00Z")]),
            g("p0", vec![]),
            g("p2", vec![skill("b", "2026-10-14T11:00:00Z")]),
        ];
        sort_groups(&mut groups);
        let ids: Vec<_> = groups.iter().map(|g| g.pane_id.as_str()).collect();
        assert_eq!(ids, vec!["p2", "p1", "p0", "p3"]);
    }

    struct Env {
        _dir: tempfile::TempDir,
        projects: PathBuf,
        app: App,
        state: PathBuf,
    }

    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        fs::create_dir_all(projects.join("-repo")).unwrap();
        fs::write(
            projects.join("-repo/sess-1.jsonl"),
            include_str!("../tests/fixtures/transcript.jsonl"),
        )
        .unwrap();
        let state = dir.path().join("state");
        let app = App::new(
            "w1:p3".into(),
            Store::new(state.clone()),
            Feeds::new(projects.clone()),
        );
        Env {
            _dir: dir,
            projects,
            app,
            state,
        }
    }

    fn snapshot() -> Snapshot {
        parse_snapshot(include_str!("../tests/fixtures/snapshot.json")).unwrap()
    }

    #[test]
    fn refresh_builds_groups_for_own_workspace() {
        let mut e = env();
        let view = e.app.refresh(Ok(snapshot()), Utc::now());
        assert_eq!(view.title, "core-8920-store-blockers");
        assert_eq!(view.status, Status::Ok);
        let ids: Vec<_> = view.groups.iter().map(|g| g.pane_id.as_str()).collect();
        // p1 has skills; p2/p5 are Claude with no usable session; w2 excluded;
        // codex pane p4 and our own pane p3 excluded.
        assert_eq!(ids, vec!["w1:p1", "w1:p2", "w1:p5"]);
        let p1 = &view.groups[0];
        assert_eq!(p1.title.as_deref(), Some("store blockers"));
        assert_eq!(p1.rows.len(), 5);
        // Same-timestamp skills keep file order, so the later one is newest.
        assert_eq!(p1.rows[0], skill("simplify", "2026-10-14T21:50:00.000Z"));
        assert_eq!(
            p1.rows[1],
            skill(
                "superpowers:test-driven-development",
                "2026-10-14T21:50:00.000Z"
            )
        );
        assert!(view.groups[1].rows.is_empty());
    }

    #[test]
    fn history_survives_session_change() {
        let mut e = env();
        e.app.refresh(Ok(snapshot()), Utc::now());

        // `/clear` in p1: herdr now reports sess-2.
        fs::write(
            e.projects.join("-repo/sess-2.jsonl"),
            r#"{"type":"assistant","timestamp":"2026-10-14T23:00:00Z","message":{"content":[{"type":"tool_use","id":"toolu_Z","name":"Skill","input":{"skill":"verification-before-completion"}}]}}
"#,
        )
        .unwrap();
        let mut snap = snapshot();
        snap.agents[0].agent_session = Some(AgentSession {
            kind: "id".into(),
            value: "sess-2".into(),
        });
        let view = e.app.refresh(Ok(snap), Utc::now());
        let rows = &view.groups[0].rows;
        assert_eq!(
            rows[0],
            skill("verification-before-completion", "2026-10-14T23:00:00Z")
        );
        assert_eq!(rows[1], Row::NewSession);
        assert_eq!(rows.len(), 7);
    }

    #[test]
    fn transcript_appearing_later_is_picked_up() {
        let mut e = env();
        let mut snap = snapshot();
        snap.agents[0].agent_session = Some(AgentSession {
            kind: "id".into(),
            value: "sess-late".into(),
        });
        let view = e.app.refresh(Ok(snap.clone()), Utc::now());
        assert!(view.groups.iter().all(|g| g.rows.is_empty()));
        fs::write(
            e.projects.join("-repo/sess-late.jsonl"),
            r#"{"type":"assistant","timestamp":"2026-10-14T23:00:00Z","message":{"content":[{"type":"tool_use","id":"toolu_L","name":"Skill","input":{"skill":"late"}}]}}
"#,
        )
        .unwrap();
        let view = e.app.refresh(Ok(snap), Utc::now());
        assert_eq!(
            view.groups[0].rows,
            vec![skill("late", "2026-10-14T23:00:00Z")]
        );
    }

    #[test]
    fn unreachable_keeps_last_view_and_registry() {
        let mut e = env();
        let good = e.app.refresh(Ok(snapshot()), Utc::now());

        let failed = e.app.refresh(Err("boom".into()), Utc::now());
        assert_eq!(failed.status, Status::Unreachable);
        assert_eq!(failed.groups, good.groups);

        // A snapshot without our own pane (e.g. herdr mid-restart, empty
        // pane list) must not prune the history.
        let mut empty = snapshot();
        empty.panes.clear();
        empty.agents.clear();
        let view = e.app.refresh(Ok(empty), Utc::now());
        assert_eq!(view.status, Status::Unreachable);
        let registry = Store::new(e.state.clone()).update(|_| false).unwrap();
        assert_eq!(registry.sessions_for("w1:p1").len(), 1);
    }

    #[test]
    fn unreachable_before_any_good_pass() {
        let mut e = env();
        let view = e.app.refresh(Err("boom".into()), Utc::now());
        assert_eq!(view.status, Status::Unreachable);
        assert!(view.groups.is_empty());
    }
}
```

Add `mod timeline;` to `src/main.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test timeline::`
Expected: compile errors (`cannot find type `App``, `Feeds`, `Row`, …).

- [ ] **Step 3: Add the implementation** at the top of `src/timeline.rs`:

```rust
//! One refresh pass: snapshot + registry + transcripts -> a `View` to draw.

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::herdr::Snapshot;
use crate::parse::{Parser, SkillEvent};
use crate::registry::{Registry, Store};
use crate::transcript::{Tail, find_transcript};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Ok,
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Skill { name: String, at: DateTime<Utc> },
    NewSession,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub pane_id: String,
    pub title: Option<String>,
    /// Newest first. Empty means "(no skills yet)".
    pub rows: Vec<Row>,
}

impl Group {
    pub fn latest(&self) -> Option<DateTime<Utc>> {
        self.rows.iter().find_map(|r| match r {
            Row::Skill { at, .. } => Some(*at),
            Row::NewSession => None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// Workspace label.
    pub title: String,
    pub status: Status,
    pub groups: Vec<Group>,
}

/// Rows for one pane, newest first. `sessions` is oldest session first; a
/// `NewSession` divider separates sessions that have events.
pub fn group_rows(sessions: &[Vec<SkillEvent>]) -> Vec<Row> {
    let mut rows = vec![];
    for events in sessions.iter().rev().filter(|e| !e.is_empty()) {
        if !rows.is_empty() {
            rows.push(Row::NewSession);
        }
        let mut sorted = events.clone();
        sorted.sort_by_key(|e| e.timestamp);
        rows.extend(sorted.into_iter().rev().map(|e| Row::Skill {
            name: e.name,
            at: e.timestamp,
        }));
    }
    rows
}

/// Most recent activity first; panes with no skills last, by pane id.
pub fn sort_groups(groups: &mut [Group]) {
    groups.sort_by(|a, b| {
        b.latest()
            .cmp(&a.latest())
            .then_with(|| a.pane_id.cmp(&b.pane_id))
    });
}

/// Parsed events per session, read incrementally across passes.
pub struct Feeds {
    projects_dir: PathBuf,
    feeds: HashMap<String, Feed>,
}

#[derive(Default)]
struct Feed {
    tail: Option<Tail>,
    parser: Parser,
    events: Vec<SkillEvent>,
}

impl Feeds {
    pub fn new(projects_dir: PathBuf) -> Self {
        Self {
            projects_dir,
            feeds: HashMap::new(),
        }
    }

    /// All events for `session_id` so far, reading any newly appended lines.
    /// A transcript that doesn't exist yet yields no events and is looked
    /// for again next time.
    pub fn events(&mut self, session_id: &str) -> &[SkillEvent] {
        let feed = self.feeds.entry(session_id.to_string()).or_default();
        if feed.tail.is_none() {
            feed.tail = find_transcript(&self.projects_dir, session_id).map(Tail::new);
        }
        if let Some(tail) = feed.tail.as_mut()
            && let Ok(lines) = tail.read_new_lines()
        {
            for line in lines {
                feed.events.extend(feed.parser.feed(&line));
            }
        }
        &feed.events
    }
}

/// State carried between refresh passes of the TUI.
pub struct App {
    own_pane: String,
    store: Store,
    feeds: Feeds,
    last_view: Option<View>,
}

impl App {
    pub fn new(own_pane: String, store: Store, feeds: Feeds) -> Self {
        Self {
            own_pane,
            store,
            feeds,
            last_view: None,
        }
    }

    /// One pass. On a failed snapshot, or one that doesn't contain this
    /// pane, keeps the last good groups, marks the view unreachable, and
    /// does not touch the registry.
    pub fn refresh(&mut self, snapshot: Result<Snapshot, String>, now: DateTime<Utc>) -> View {
        let view = snapshot.ok().and_then(|snap| {
            let ws = snap.workspace_of(&self.own_pane)?.to_string();
            Some(self.build(&snap, &ws, now))
        });
        match view {
            Some(view) => {
                self.last_view = Some(view.clone());
                view
            }
            None => {
                let mut stale = self.last_view.clone().unwrap_or(View {
                    title: String::new(),
                    status: Status::Ok,
                    groups: vec![],
                });
                stale.status = Status::Unreachable;
                stale
            }
        }
    }

    fn build(&mut self, snap: &Snapshot, workspace_id: &str, now: DateTime<Utc>) -> View {
        let all_claude = snap.claude_panes();
        let live = snap.live_pane_ids();
        let record_all = |r: &mut Registry| {
            let mut changed = false;
            for pane in &all_claude {
                if let Some(session) = &pane.session_id {
                    changed |= r.record(&pane.pane_id, session, now);
                }
            }
            changed | r.prune(&live)
        };
        // If the state file can't be used, fall back to this pass's sessions
        // so the current session still shows.
        let registry = self.store.update(record_all).unwrap_or_else(|e| {
            eprintln!("herdr-skill-timeline: state file unavailable: {e}");
            let mut r = Registry::default();
            record_all(&mut r);
            r
        });

        let mut groups: Vec<Group> = snap
            .claude_panes_in(workspace_id)
            .into_iter()
            .map(|pane| {
                let sessions: Vec<Vec<SkillEvent>> = registry
                    .sessions_for(&pane.pane_id)
                    .iter()
                    .map(|s| self.feeds.events(&s.session_id).to_vec())
                    .collect();
                Group {
                    rows: group_rows(&sessions),
                    pane_id: pane.pane_id,
                    title: pane.title,
                }
            })
            .collect();
        sort_groups(&mut groups);

        View {
            title: snap
                .workspace_label(workspace_id)
                .unwrap_or(workspace_id)
                .to_string(),
            status: Status::Ok,
            groups,
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test timeline::`
Expected: `test result: ok. 7 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/timeline.rs src/main.rs
git commit -m "feat: build grouped timeline view from sessions and transcripts"
```

---

### Task 7: `record` event-hook mode

**Files:**
- Create: `src/record.rs`
- Modify: `src/main.rs` (add `mod record;`)

**Interfaces:**
- Consumes:
  - From Task 4: `Snapshot`, `fetch_snapshot`, `parse_snapshot` (tests).
  - From Task 5: `Store`.
- Produces:
  - `record::run()`, which reads `HERDR_PLUGIN_EVENT_JSON`, never panics and never prints to stdout.
  - `record::pane_id_from_event(json: &str) -> Option<String>`.
  - `record::record_pane(pane_id: &str, snap: &Snapshot, store: &Store, now) -> io::Result<bool>`.

herdr passes event hooks the event name in `HERDR_PLUGIN_EVENT` and the payload in `HERDR_PLUGIN_EVENT_JSON`, for example `{"event":"pane.agent_status_changed","data":{"pane_id":"w1:p1",…}}`. The payload doesn't reliably carry the session ID, so the hook looks it up in a fresh snapshot.

- [ ] **Step 1: Write the failing tests.** Create `src/record.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::parse_snapshot;

    fn snapshot() -> Snapshot {
        parse_snapshot(include_str!("../tests/fixtures/snapshot.json")).unwrap()
    }

    #[test]
    fn extracts_pane_id() {
        assert_eq!(
            pane_id_from_event(
                r#"{"event":"pane.agent_detected","data":{"pane_id":"w1:p1","agent":"claude"}}"#
            ),
            Some("w1:p1".into())
        );
        assert_eq!(pane_id_from_event(r#"{"data":{}}"#), None);
        assert_eq!(pane_id_from_event(r#"{"data":{"pane_id":""}}"#), None);
        assert_eq!(pane_id_from_event("garbage"), None);
    }

    #[test]
    fn records_claude_pane_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf());
        assert!(record_pane("w1:p1", &snapshot(), &store, Utc::now()).unwrap());
        assert!(!record_pane("w1:p1", &snapshot(), &store, Utc::now()).unwrap());
        let r = store.update(|_| false).unwrap();
        assert_eq!(r.sessions_for("w1:p1")[0].session_id, "sess-1");
    }

    #[test]
    fn ignores_non_claude_and_sessionless_panes_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let store = Store::new(state.clone());
        for pane in ["w1:p4", "w1:p2", "w1:p3", "missing"] {
            assert!(!record_pane(pane, &snapshot(), &store, Utc::now()).unwrap());
        }
        assert!(!state.exists());
    }
}
```

Add `mod record;` to `src/main.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test record::`
Expected: compile errors (`cannot find function `pane_id_from_event``, `record_pane`).

- [ ] **Step 3: Add the implementation** at the top of `src/record.rs`:

```rust
//! `record` mode: the herdr event hook. Saves the event's pane's current
//! Claude session so history survives `/clear` even while the TUI is closed.
//! Always exits 0 and prints nothing to stdout.

use std::env;
use std::io;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::herdr::{Snapshot, fetch_snapshot};
use crate::registry::Store;

pub fn run() {
    let Ok(json) = env::var("HERDR_PLUGIN_EVENT_JSON") else {
        return;
    };
    let Some(pane_id) = pane_id_from_event(&json) else {
        return;
    };
    let result = fetch_snapshot().and_then(|snap| {
        record_pane(&pane_id, &snap, &Store::default_location(), Utc::now())
            .map_err(|e| e.to_string())
    });
    if let Err(e) = result {
        eprintln!("herdr-skill-timeline record: {e}");
    }
}

/// `data.pane_id` from a herdr plugin event payload.
pub fn pane_id_from_event(json: &str) -> Option<String> {
    let event: Value = serde_json::from_str(json).ok()?;
    event
        .pointer("/data/pane_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Records the pane's session if it is a Claude pane with a known session.
/// Returns true if the registry changed. Non-Claude panes never touch disk.
pub fn record_pane(
    pane_id: &str,
    snap: &Snapshot,
    store: &Store,
    now: DateTime<Utc>,
) -> io::Result<bool> {
    let Some(session_id) = snap
        .claude_panes()
        .into_iter()
        .find(|p| p.pane_id == pane_id)
        .and_then(|p| p.session_id)
    else {
        return Ok(false);
    };
    let mut changed = false;
    store.update(|r| {
        changed = r.record(pane_id, &session_id, now);
        changed
    })?;
    Ok(changed)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test record::`
Expected: `test result: ok. 3 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/record.rs src/main.rs
git commit -m "feat: record mode for herdr event hook"
```

---

### Task 8: Rendering

**Files:**
- Create: `src/ui.rs`
- Modify: `src/main.rs` (add `mod ui;`)

**Interfaces:**
- Consumes:
  - From Task 1: `ago::label`.
  - From Task 6: `View`, `Group`, `Row`, `Status`.
- Produces:
  - `ui::render(frame: &mut ratatui::Frame, view: &View, now: DateTime<Utc>, scroll: &mut u16)`, which clamps `scroll` to the content height.
  - `ui::lines(view: &View, now, width: usize) -> Vec<Line<'static>>`.

Layout rules from the spec:
- The header shows the workspace label, with `⟳ 2s` or `⚠ herdr unreachable` on the right.
- Each group heading is `─ claude · <short pane id> "<title>" ───`.
- Rows are indented three spaces, with the time right-aligned and never truncated, and the name cut with `…` when space runs out.
- The newest row of each group is bold.
- A group with no skills shows `(no skills yet)`.
- Sessions are separated by `┄┄ new session ┄┄`.

- [ ] **Step 1: Write the failing tests.** Create `src/ui.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    const NOW: &str = "2026-10-14T12:00:00Z";

    fn view() -> View {
        View {
            title: "core-8920".into(),
            status: Status::Ok,
            groups: vec![
                Group {
                    pane_id: "w1:p2".into(),
                    title: Some("store blockers".into()),
                    rows: vec![
                        Row::Skill {
                            name: "simplify".into(),
                            at: t("2026-10-14T11:57:00Z"),
                        },
                        Row::Skill {
                            name: "test-driven-development".into(),
                            at: t("2026-10-14T11:42:00Z"),
                        },
                        Row::NewSession,
                        Row::Skill {
                            name: "superpowers:brainstorming".into(),
                            at: t("2026-10-14T10:00:00Z"),
                        },
                    ],
                },
                Group {
                    pane_id: "w1:p4".into(),
                    title: None,
                    rows: vec![],
                },
            ],
        }
    }

    fn draw(view: &View, width: u16, height: u16, scroll: &mut u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| render(f, view, t(NOW), scroll)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn text(buf: &Buffer) -> Vec<String> {
        let width = buf.area.width as usize;
        buf.content
            .chunks(width)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect()
    }

    #[test]
    fn renders_groups_rows_and_dividers() {
        let buf = draw(&view(), 44, 9, &mut 0);
        assert_eq!(
            text(&buf),
            vec![
                " Skill timeline · core-8920             ⟳ 2s",
                "─ claude · p2 \"store blockers\" ─────────────",
                "   simplify                        3 min ago",
                "   test-driven-development        18 min ago",
                "   ┄┄ new session ┄┄                        ",
                "   superpowers:brainstorming        2 hr ago",
                "─ claude · p4 ──────────────────────────────",
                "   (no skills yet)                          ",
                "                                            ",
            ]
        );
    }

    #[test]
    fn newest_row_is_bold_and_others_are_not() {
        let buf = draw(&view(), 44, 9, &mut 0);
        assert!(buf[(3, 2)].modifier.contains(Modifier::BOLD));
        assert!(!buf[(3, 3)].modifier.contains(Modifier::BOLD));
        assert!(!buf[(3, 5)].modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn narrow_pane_truncates_names_not_times() {
        let lines = text(&draw(&view(), 24, 9, &mut 0));
        assert_eq!(lines[3], "   test-driv… 18 min ago");
        assert_eq!(lines[5], "   superpowers… 2 hr ago");
    }

    #[test]
    fn tiny_pane_does_not_panic() {
        for width in 0..6 {
            draw(&view(), width, 3, &mut 0);
        }
    }

    #[test]
    fn unreachable_header() {
        let mut v = view();
        v.status = Status::Unreachable;
        let lines = text(&draw(&v, 44, 2, &mut 0));
        assert_eq!(lines[0], " Skill timeline · core-… ⚠ herdr unreachable");
    }

    #[test]
    fn scroll_is_clamped_to_content() {
        let mut scroll = 100;
        let buf = draw(&view(), 44, 4, &mut scroll);
        assert_eq!(scroll, 4); // 8 lines of content, 4 visible
        assert_eq!(
            text(&buf)[3],
            "   (no skills yet)                          "
        );
    }
}
```

Add `mod ui;` to `src/main.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test ui::`
Expected: compile errors (`cannot find function `render``).

- [ ] **Step 3: Add the implementation** at the top of `src/ui.rs`:

```rust
//! Drawing a `View` with ratatui.

use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::ago;
use crate::timeline::{Group, Row, Status, View};

const INDENT: &str = "   ";

/// Draws `view`, clamping `scroll` to the content height.
pub fn render(frame: &mut Frame, view: &View, now: DateTime<Utc>, scroll: &mut u16) {
    let area = frame.area();
    let lines = lines(view, now, area.width as usize);
    let max_scroll = lines.len().saturating_sub(area.height as usize);
    *scroll = (*scroll).min(u16::try_from(max_scroll).unwrap_or(u16::MAX));
    frame.render_widget(Paragraph::new(lines).scroll((*scroll, 0)), area);
}

/// Every line of the timeline at `width` columns.
pub fn lines(view: &View, now: DateTime<Utc>, width: usize) -> Vec<Line<'static>> {
    let mut out = vec![header(view, width)];
    for group in &view.groups {
        out.push(Line::from(heading(group, width)));
        out.extend(group_lines(group, now, width));
    }
    out
}

fn header(view: &View, width: usize) -> Line<'static> {
    let right = match view.status {
        Status::Ok => "⟳ 2s",
        Status::Unreachable => "⚠ herdr unreachable",
    };
    let left = if view.title.is_empty() {
        " Skill timeline".to_string()
    } else {
        format!(" Skill timeline · {}", view.title)
    };
    Line::from(spread(&left, right, width)).style(Style::new().add_modifier(Modifier::BOLD))
}

fn heading(group: &Group, width: usize) -> String {
    let short_id = group.pane_id.rsplit(':').next().unwrap_or(&group.pane_id);
    let mut text = format!("─ claude · {short_id}");
    if let Some(title) = &group.title {
        text.push_str(&format!(" \"{title}\""));
    }
    text.push(' ');
    let text = fit(&text, width);
    let fill = width.saturating_sub(text.chars().count());
    format!("{text}{}", "─".repeat(fill))
}

fn group_lines(group: &Group, now: DateTime<Utc>, width: usize) -> Vec<Line<'static>> {
    if group.rows.is_empty() {
        return vec![Line::from(fit(&format!("{INDENT}(no skills yet)"), width))];
    }
    let mut newest_done = false;
    group
        .rows
        .iter()
        .map(|row| match row {
            Row::NewSession => Line::from(fit(&format!("{INDENT}┄┄ new session ┄┄"), width)),
            Row::Skill { name, at } => {
                let line = Line::from(spread(
                    &format!("{INDENT}{name}"),
                    &ago::label(*at, now),
                    width,
                ));
                if newest_done {
                    line
                } else {
                    newest_done = true;
                    line.style(Style::new().add_modifier(Modifier::BOLD))
                }
            }
        })
        .collect()
}

/// `left` and `right` on one line of `width` columns: `right` flush right and
/// never truncated (unless it alone exceeds the width); `left` truncated
/// with `…` to make room.
fn spread(left: &str, right: &str, width: usize) -> String {
    let right_len = right.chars().count();
    if right_len + 1 > width {
        return fit(right, width);
    }
    let left = fit(left, width - right_len - 1);
    let gap = width - right_len - left.chars().count();
    format!("{left}{}{right}", " ".repeat(gap))
}

/// `s` cut to at most `max` characters, ending in `…` if cut.
fn fit(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut cut: String = s.chars().take(max - 1).collect();
    cut.push('…');
    cut
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test ui::`
Expected: `test result: ok. 6 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/ui.rs src/main.rs
git commit -m "feat: render timeline with ratatui"
```

---

### Task 9: Binary entry point, plugin manifest, install, and acceptance

**Files:**
- Modify: `src/main.rs` (final version)
- Create: `herdr-plugin.toml`, `scripts/install.sh`, `README.md`

**Interfaces:**
- Consumes:
  - From Task 3: `transcript::projects_dir`.
  - From Task 4: `herdr::fetch_snapshot`.
  - From Task 5: `Store::default_location`.
  - From Task 6: `App`, `Feeds`.
  - From Task 7: `record::run`.
  - From Task 8: `ui::render`.
- Produces: the `herdr-skill-timeline` binary. With no arguments it runs the TUI; `record` runs the hook; any other argument prints usage and exits 2.

- [ ] **Step 1: Replace `src/main.rs`** with the final version:

```rust
mod ago;
mod herdr;
mod parse;
mod record;
mod registry;
mod timeline;
mod transcript;
mod ui;

use std::env;
use std::io;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use chrono::Utc;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::registry::Store;
use crate::timeline::{App, Feeds};

const REFRESH: Duration = Duration::from_secs(2);

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => run_tui(),
        Some("record") => {
            record::run();
            ExitCode::SUCCESS
        }
        Some(_) => {
            eprintln!("usage: herdr-skill-timeline [record]");
            ExitCode::from(2)
        }
    }
}

fn run_tui() -> ExitCode {
    let in_herdr = env::var("HERDR_ENV").is_ok_and(|v| v == "1");
    let own_pane = env::var("HERDR_PANE_ID").ok().filter(|p| !p.is_empty());
    let (true, Some(own_pane)) = (in_herdr, own_pane) else {
        eprintln!("herdr-skill-timeline: not running inside a herdr pane");
        return ExitCode::FAILURE;
    };

    let mut app = App::new(
        own_pane,
        Store::default_location(),
        Feeds::new(transcript::projects_dir()),
    );
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("herdr-skill-timeline: {e}");
            ExitCode::FAILURE
        }
    }
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    let mut scroll: u16 = 0;
    let mut view = app.refresh(herdr::fetch_snapshot(), Utc::now());
    let mut last_refresh = Instant::now();
    loop {
        terminal.draw(|frame| ui::render(frame, &view, Utc::now(), &mut scroll))?;

        // Wait for a key/resize, or until the next refresh is due.
        if event::poll(REFRESH.saturating_sub(last_refresh.elapsed()))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(());
                }
                KeyCode::Char('j') | KeyCode::Down => scroll = scroll.saturating_add(1),
                KeyCode::Char('k') | KeyCode::Up => scroll = scroll.saturating_sub(1),
                _ => {}
            }
        }

        if last_refresh.elapsed() >= REFRESH {
            view = app.refresh(herdr::fetch_snapshot(), Utc::now());
            last_refresh = Instant::now();
        }
    }
}
```

- [ ] **Step 2: Verify the whole crate**

Run each:
- `cargo test`. Expected: `test result: ok. 33 passed`.
- `cargo clippy --all-targets`. Expected: no warnings.
- `cargo fmt --check`. Expected: no output.

- [ ] **Step 3: Verify the outside-herdr guard**

Run: `env -u HERDR_ENV -u HERDR_PANE_ID cargo run -q; echo "exit=$?"`
Expected: `herdr-skill-timeline: not running inside a herdr pane` then `exit=1`.

Run: `cargo run -q -- bogus; echo "exit=$?"`
Expected: `usage: herdr-skill-timeline [record]` then `exit=2`.

Run: `env -u HERDR_PLUGIN_EVENT_JSON cargo run -q -- record; echo "exit=$?"`
Expected: no output, `exit=0`.

- [ ] **Step 4: Create the plugin manifest** `herdr-plugin.toml`:

```toml
# herdr-skill-timeline: timeline of Claude skills per pane in the current
# workspace. The TUI is started by the user in any pane (e.g. from a
# workspace-manager layout); this manifest only builds the binary and wires
# the event hook that remembers each pane's Claude sessions across /clear.

id = "herdr-skill-timeline"
name = "Skill Timeline"
version = "0.1.0"
description = "Timeline of Claude skills run by the sessions in the current workspace."
min_herdr_version = "0.9.0"
platforms = ["linux", "macos"]

[[build]]
command = ["cargo", "build", "--release"]

[[events]]
on = "pane.agent_detected"
command = ["./target/release/herdr-skill-timeline", "record"]

[[events]]
on = "pane.agent_status_changed"
command = ["./target/release/herdr-skill-timeline", "record"]
```

- [ ] **Step 5: Create** `scripts/install.sh` and make it executable (`chmod +x scripts/install.sh`):

```sh
#!/bin/sh
# Symlink the release binary into ~/.local/bin so pane commands can run
# `herdr-skill-timeline` by name.
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"
bin="$root/target/release/herdr-skill-timeline"

[ -x "$bin" ] || cargo build --release --manifest-path "$root/Cargo.toml"

mkdir -p "$HOME/.local/bin"
ln -sf "$bin" "$HOME/.local/bin/herdr-skill-timeline"
echo "linked $HOME/.local/bin/herdr-skill-timeline -> $bin"

case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) echo "note: $HOME/.local/bin is not on PATH" >&2 ;;
esac
```

- [ ] **Step 6: Create** `README.md`:

````markdown
# herdr-skill-timeline

A [herdr](https://herdr.dev) pane that shows which Claude Code skills each
Claude session in the current workspace has run, grouped by pane, newest
first, with "10 min ago" labels. History for a pane survives `/clear`.

```
 Skill timeline · core-8920-store-blockers            ⟳ 2s
 ─ claude · p2 "store blockers" ───────────────────────────
   simplify                               3 min ago
   test-driven-development               18 min ago
   ┄┄ new session ┄┄
   superpowers:brainstorming              2 hr ago
```

Both ways of running a skill are tracked: Claude calling the `Skill` tool, and
you typing `/skill-name`. Built-in commands like `/clear` are ignored.

## Install

Requires a Rust toolchain (1.89+).

```sh
git clone <this repo> ~/dev/herdr-skill-timeline
herdr plugin link ~/dev/herdr-skill-timeline   # builds, registers the event hook
~/dev/herdr-skill-timeline/scripts/install.sh  # puts the binary on PATH
```

## Use

Run `herdr-skill-timeline` in any pane of the workspace. With
herdr-plugin-workspace-manager:

```yaml
panes:
  - agent: claude
  - command: herdr-skill-timeline
    persist: false
    size: "25%"
```

Keys: `j`/`k` or arrows scroll, `q` quits.

## How it works

- Every 2s it reads `herdr api snapshot`, finds the Claude panes in its own
  workspace, and reads new lines from each session's transcript in
  `~/.claude/projects/*/<session_id>.jsonl`.
- A herdr event hook (`record`) saves each pane's session ID to
  `~/.local/state/herdr-skill-timeline/sessions.json`, so earlier sessions in
  that pane stay visible after `/clear`, even if the timeline wasn't open.

## Development

```sh
cargo test
cargo clippy --all-targets
```
````

- [ ] **Step 7: Commit**

```bash
git add src/main.rs herdr-plugin.toml scripts/install.sh README.md
git commit -m "feat: TUI entry point, herdr plugin manifest, install script"
```

- [ ] **Step 8: Install** (this changes the user's herdr config and `~/.local/bin`, so confirm with the user before running it)

```bash
herdr plugin link ~/dev/herdr-skill-timeline
herdr plugin list --plugin herdr-skill-timeline
./scripts/install.sh
command -v herdr-skill-timeline
```
Expected:
- `plugin list` shows `herdr-skill-timeline`, enabled.
- `command -v` prints `~/.local/bin/herdr-skill-timeline`.

If `~/.local/bin` isn't on `PATH`, tell the user rather than editing their shell config.

- [ ] **Step 9: Manual acceptance** (the user does this in herdr; the implementer reports the steps and waits)

1. In a workspace with a Claude pane, open a new pane and run `herdr-skill-timeline`. Expected: the header shows the workspace label, and the Claude pane appears as a group.
2. In the Claude pane, ask Claude to use a skill (e.g. "use the simplify skill"), then type `/simplify`. Expected: two `simplify` rows appear within about 2s, and the newest is bold.
3. Quit the timeline (`q`). In the Claude pane run `/clear`, then `/simplify`. Reopen `herdr-skill-timeline`. Expected: the new row is at the top, followed by `┄┄ new session ┄┄`, with the earlier rows still below.
4. Run `herdr-skill-timeline` in a terminal outside herdr. Expected: the error message and exit status 1.
