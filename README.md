# herdr-skill-timeline

A [herdr](https://herdr.dev) pane that shows which Claude Code skills each
Claude session in the current workspace has run, and the pull requests it
created and merged, grouped by pane, newest first, with "10 min ago" labels.
History for a pane survives `/clear`.

```
 Event timeline · core-8920-store-blockers            ⟳ 2s
 ─ claude · store blockers ────────────────────────────────
   PR 2229 merged                         1 min ago
   simplify                               3 min ago
   PR 2229 created                       12 min ago
   test-driven-development               18 min ago
   ┄┄ new session ┄┄
   superpowers:brainstorming              2 hr ago
```

Both ways of running a skill are tracked: Claude calling the `Skill` tool, and
you typing `/skill-name`. Built-in commands like `/clear` are ignored.

Pull requests show as "PR 1234 created" and "PR 1234 merged" when Claude runs
`gh pr create` or `gh pr merge` and the command succeeds. The number comes from
the PR link `gh pr create` prints, and from `gh pr merge`'s argument (or its
output). PRs created or merged outside the session, e.g. on github.com, don't
show.

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
cargo fmt --check
```

CI runs all three on every pull request, with warnings counted as errors,
and `main` requires it to pass before merging.
