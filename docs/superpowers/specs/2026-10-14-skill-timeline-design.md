# herdr-skill-timeline — Design

Date: 2026-10-14
Status: Approved design, pending implementation plan

## Purpose

When several Claude Code sessions run in herdr, it is easy to lose track of which
development phase each one is in. Phases are marked by the skills that get run
(brainstorming → writing-plans → test-driven-development → simplify → …).
Re-orienting currently means scrolling back through each session.

`herdr-skill-timeline` is a TUI that runs in a herdr pane and shows, at a glance,
which skills each Claude session in the current workspace has run, newest first,
with "10 min ago" style labels.

### Success criteria

- A skill invoked in any Claude pane of the workspace appears in the timeline
  within ~2 seconds, whether Claude invoked it (`Skill` tool) or the user typed it
  as a slash command.
- History for a pane survives `/clear` and Claude restarts in that pane, including
  when the timeline pane was not open at the time.
- The newest skill per session is visually obvious ("current phase").

### Non-goals (v1)

- Workflow / phase matching ("what's left to do"). Timeline only.
- Agents other than Claude Code.
- Skills invoked inside subagents (sidechain transcripts).
- Opening or managing its own pane. Layout is the user's responsibility
  (e.g. via herdr-plugin-workspace-manager).
- Config files, colour themes, mouse/click handling.

## Context: what herdr and Claude Code already provide

- herdr's Claude integration (`~/.claude/hooks/herdr-agent-state.sh`) reports each
  pane's Claude `session_id` on `SessionStart`. It is visible in
  `herdr api snapshot` at `result.snapshot.agents[].agent_session.value`
  (`kind: "id"`), alongside `pane_id`, `workspace_id`, `terminal_title`.
- Every pane herdr spawns has `HERDR_ENV=1`, `HERDR_PANE_ID`, `HERDR_WORKSPACE_ID`,
  `HERDR_SOCKET_PATH`, `HERDR_BIN_PATH` in its environment.
- Plugin event hooks receive `HERDR_PLUGIN_EVENT` (event name) and
  `HERDR_PLUGIN_EVENT_JSON` (payload with `data.pane_id`).
- Claude transcripts live at `${CLAUDE_CONFIG_DIR:-~/.claude}/projects/*/<session_id>.jsonl`,
  one JSON object per line, each with an ISO-8601 `timestamp`.

## Deliverable layout

Repository: `~/dev/herdr-skill-timeline`

```
herdr-plugin.toml        # build + events only; no panes, no actions
Cargo.toml
scripts/install.sh       # symlink target/release/herdr-skill-timeline into ~/.local/bin
src/
  main.rs                # mode dispatch: TUI (default) | record
  herdr.rs               # snapshot fetch + parse
  registry.rs            # sessions.json read/modify/prune/write
  transcript.rs          # locate + incrementally read transcript files
  parse.rs               # transcript line -> skill events
  ago.rs                 # relative time labels
  timeline.rs            # one refresh pass -> View
  record.rs              # event-hook mode
  ui.rs                  # ratatui rendering
tests/fixtures/          # sanitized transcript lines, snapshot JSON
```

Installation: `herdr plugin link ~/dev/herdr-skill-timeline` (herdr runs the
build and registers the event hook), then `scripts/install.sh` so the binary is
on `PATH`. Usage from a workspace-manager layout:

```yaml
panes:
  - agent: claude
  - command: herdr-skill-timeline
    persist: false
    size: "25%"
```

### Manifest

```toml
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

## Components

| Unit | Responsibility | Depends on |
|---|---|---|
| `herdr.rs` | Run `$HERDR_BIN_PATH api snapshot` (fallback `herdr`), parse into `Snapshot { workspaces, panes, agents }`. Helpers: workspace of a pane; Claude agents in a workspace with `pane_id`, `session_id`, `terminal_title`. | herdr CLI |
| `registry.rs` | Persist which sessions each pane has had. `record(pane_id, session_id, now)`; `prune(live_pane_ids)`; `sessions_for(pane_id)`. | filesystem |
| `transcript.rs` | Resolve `<session_id>.jsonl` under `${CLAUDE_CONFIG_DIR:-~/.claude}/projects/*/` (cache resolved path). Keep a byte offset per file; return only newly completed lines each pass. | filesystem |
| `parse.rs` | Pure: feed lines in order, emit `SkillEvent { name, timestamp, via: Tool \| Slash }`. Holds pending-slash state between lines. | none |
| `ago.rs` | Pure: `label(ts, now) -> String`. | none |
| `timeline.rs` | One refresh pass: record + prune sessions, read feeds, build the `View` (groups, rows, ordering). Keeps the last good view for the unreachable state. | herdr, registry, transcript, parse |
| `record.rs` | The `record` event-hook mode. | herdr, registry |
| `ui.rs` | Pure-ish rendering of a `View` model to a ratatui `Frame`; key handling (scroll, quit). | ratatui, crossterm |
| `main.rs` | Mode dispatch and the 2-second loop. | all |

## Data model

### `sessions.json`

Location: `${XDG_STATE_HOME:-~/.local/state}/herdr-skill-timeline/sessions.json`.
A fixed path is used deliberately: the TUI runs in a user pane and does not
receive `HERDR_PLUGIN_STATE_DIR`, so both modes must agree on a path
independent of herdr's per-plugin state dir.

```json
{
  "version": 1,
  "panes": {
    "w4K:p2": [
      { "session_id": "aa963ac4-…", "first_seen": "2026-10-14T21:22:59Z" },
      { "session_id": "0b1c…",      "first_seen": "2026-10-14T23:05:12Z" }
    ]
  }
}
```

Keyed by pane id only; the workspace a pane belongs to is always taken from
the live snapshot, so a pane moved between workspaces keeps its history.

Session lists are append-only (deduplicated by `session_id`), ordered by
`first_seen`.

### In-memory

```rust
struct SkillEvent { name: String, timestamp: DateTime<Utc>, via: Via }
enum Via { Tool, Slash }
struct SessionHistory { session_id: String, events: Vec<SkillEvent> }
struct PaneGroup { pane_id: String, title: Option<String>, sessions: Vec<SessionHistory> }
```

## Modes

### TUI (default: `herdr-skill-timeline`)

Startup:
1. If `HERDR_ENV != "1"` or `HERDR_PANE_ID` is unset, print
   `herdr-skill-timeline: not running inside a herdr pane` to stderr, exit 1.
2. Enter the alternate screen and raw mode (restored on exit and on panic).

Loop, every 2 seconds and on terminal resize:
1. Fetch the snapshot. Determine the current workspace from the own pane's
   `workspace_id` (re-evaluated each pass, so a moved pane follows its workspace).
2. For each Claude agent (any workspace) with a session id, `registry.record`.
   Then `registry.prune` with the live pane ids from the snapshot. A snapshot
   that fails, or that does not contain the TUI's own pane (e.g. herdr
   mid-restart with an empty pane list), skips this step entirely, so history
   is never pruned on bad data.
3. For every session of every live pane in the workspace, read new transcript
   lines and feed them to that session's parser; append emitted events.
4. Build the `View` and redraw (redraw also refreshes the "ago" labels).

The TUI's own pane is never listed (it is not a Claude agent).

Keys: `j`/`k`/↓/↑ scroll, `q` / Ctrl-C quit.

### `record` (event hook)

1. Read `data.pane_id` from `HERDR_PLUGIN_EVENT_JSON`. Missing → exit 0.
2. Fetch the snapshot; find that pane's agent. Not Claude or no session id → exit 0.
3. `registry.record(pane_id, session_id, now)`. Exit 0. Events for non-Claude
   panes return before touching the state file.

`record` always exits 0 and writes nothing to stdout, so it never disturbs herdr.
Errors go to stderr as one line.

## Parsing rules (`parse.rs`)

Process lines in file order. Ignore lines that fail to parse as JSON and any
entry with `isSidechain: true`.

1. **Agent-invoked:** an entry with `type == "assistant"` whose
   `message.content[]` contains a block with `type == "tool_use"` and
   `name == "Skill"` emits `SkillEvent { name: input.skill, timestamp, via: Tool }`.
   Multiple such blocks in one entry emit multiple events. A `tool_use` `id`
   already emitted is skipped (resumed/forked sessions can repeat history).
   Only `type == "assistant"` counts: transcripts also contain
   `api-request-blob` entries that repeat earlier `tool_use` blocks.
2. **User-invoked:** an entry with `type == "user"` whose `message.content`
   (string, or text blocks) contains `<command-name>/X</command-name>` sets
   `pending = Some((X, timestamp))`, replacing any earlier pending value.
3. **Confirmation:** while `pending` is set, a `type == "user"` entry with
   `isMeta == true` whose text starts with `Base directory for this skill:`
   emits `SkillEvent { name: X, timestamp: pending.timestamp, via: Slash }` and
   clears `pending`. Other meta entries (e.g. hook output) leave `pending`
   untouched. A non-meta `user` entry or any `assistant` entry clears `pending`
   without emitting (case 2 may then set it again).
   Built-in commands (`/clear`, `/model`, `/compact`, …) and plugin commands that
   do not load a skill never get this meta entry and so are never emitted.

Names are displayed exactly as invoked (e.g. `superpowers:brainstorming`).

Incomplete trailing line: `transcript.rs` only hands over lines terminated by
`\n`; the remainder stays unread until the next pass.

## Display (`ui.rs`)

```
 Skill timeline · core-8920-store-blockers            ⟳ 2s
 ─ claude · store blockers ────────────────────────────────
   simplify                               3 min ago
   test-driven-development               18 min ago
   ┄┄ new session ┄┄
   writing-plans                          1 hr ago
   superpowers:brainstorming              2 hr ago
 ─ claude ─────────────────────────────────────────────────
   (no skills yet)
```

- Header: workspace label (from the snapshot). Shows `⚠ herdr unreachable`
  in place of `⟳ 2s` while snapshots are failing.
- One group per live Claude pane in the workspace. Groups ordered by their
  most recent event, newest first; panes with no events last, by pane id.
- Group heading: `claude · <terminal_title>`, or just `claude` when the
  title is empty. The pane id is not shown (herdr does not display it).
- Within a group, events newest first across all of the pane's sessions; a
  `┄┄ new session ┄┄` divider between events of different sessions.
- The newest event in each group is bold.
- Session with no transcript file yet: no events and no error. A group with no
  events shows `(no skills yet)`.
- Narrow panes: the time column is right-aligned and never truncated; skill
  names are truncated with `…`.

### Relative time (`ago.rs`)

| Age | Label |
|---|---|
| < 1 min | `just now` |
| < 1 hr | `N min ago` |
| < 24 hr | `N hr ago` |
| < 48 hr | `yesterday` |
| otherwise | `N days ago` |

Future timestamps (clock skew) render as `just now`.

## Error handling

| Situation | Behaviour |
|---|---|
| Concurrent writers (TUI + hook) | Read-modify-write while holding an exclusive advisory lock on `sessions.json.lock` (std `File::lock`); write to a temp file and rename. Each writer applies its change to the latest on-disk data, so no record is lost. |
| `sessions.json` missing | Treat as empty. |
| `sessions.json` corrupt | Rename to `sessions.json.bak`, start empty, log one stderr line. |
| Snapshot command fails | TUI keeps the last good view and shows the warning header; `record` exits 0. |
| Transcript missing/unreadable | Session contributes no events; retried next pass. |
| Pane closed | Pruned from the registry when absent from the snapshot. |
| Workspace closed | Workspace key pruned when it has no live panes. |
| Outside herdr | Exit 1 with the message above. |

## Testing

- Unit tests (`cargo test`):
  - `parse.rs` against `tests/fixtures/*.jsonl` built from real transcript
    lines with content removed: `Skill` tool_use; slash skill + meta confirmation;
    `/clear` (no emission); slash command followed by a non-skill meta entry;
    sidechain entry; invalid JSON line.
  - `transcript.rs`: incremental reads across appends, trailing partial line.
  - `ago.rs`: each boundary (59s, 60s, 59m, 60m, 23h, 24h, 47h, 48h, future).
  - `registry.rs`: record dedup, merge of two concurrent views, prune, corrupt
    file recovery (tempdir).
  - `herdr.rs`: snapshot fixture → Claude agents for a workspace.
- Render test: ratatui `TestBackend` snapshot covering grouping order, session
  divider, empty group, bold newest, narrow-width truncation.
- Manual acceptance:
  1. Run in a pane beside a Claude session; invoke a skill via the agent and
     via `/skill`; both appear within ~2s.
  2. Close the timeline, `/clear` in the Claude pane, invoke a skill, reopen the
     timeline: earlier history is intact under a `new session` divider.
  3. Run outside herdr: prints the error and exits 1.

## Dependencies

`ratatui` 0.30, `crossterm` 0.29, `serde`, `serde_json`, `chrono`
(no default features; `std`, `clock`, `serde`), `tempfile` (dev). File locking
uses std `File::lock` (Rust ≥ 1.89), so no lock crate. Matches the versions
used by herdr-notes where shared.
