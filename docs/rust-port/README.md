# TypeSpec Rust Port — Documentation

This directory contains the planning and tracking artifacts for porting the TypeSpec compiler from TypeScript to Rust as a learning project.

## Files

| File                                       | Purpose                                                                                |
| ------------------------------------------ | -------------------------------------------------------------------------------------- |
| [`plan.md`](./plan.md)                     | The detailed plan: scope, design decisions, milestones, acceptance criteria            |
| [`tasks.json`](./tasks.json)               | Machine-readable task tracker. **This is the source of truth for "what's in flight."** |
| [`tasks.schema.json`](./tasks.schema.json) | JSON schema for `tasks.json` (editor autocomplete + validation)                        |
| `spike-napi-roundtrip.md`                  | (created during M1.5) Findings from the napi-rs round-trip spike                       |

Related: [`docs/adr/0001-arena-based-type-graph.md`](../adr/0001-arena-based-type-graph.md) — ADR for the arena allocation decision.

## Workflow for Agents

### 1. Pick a task

```bash
# Show tasks that are ready to start (status=not_started and all deps done)
jq '
  .tasks as $all
  | .tasks[]
  | select(.status == "not_started")
  | select(
      all(
        .depends_on[] as $dep
        | $all[] | select(.id == $dep) | .status == "done"
      )
    )
  | { id, title, complexity }
' docs/rust-port/tasks.json
```

Pick one. Don't take a task whose `depends_on` aren't all `done`.

### 2. Claim it

Update the task in `tasks.json`:

- `status`: `"in_progress"`
- `assignee`: your GitHub username or agent identifier
- `branch`: the git branch you'll work on (e.g., `jeremymeng/rust-port-m2-scanner`)
- `started_at`: current ISO-8601 UTC timestamp
- Bump the top-level `updated_at` and set `updated_by`

Commit this update on your branch immediately so other agents see it.

### 3. Do the work

Read `plan.md` at the section linked from `plan_section`. Treat the TS source files in `key_source_files` as the specification.

As you complete checklist items, tick them off (`"done": true`). Commit checklist updates along with the code that satisfies them — reviewers can verify each tick.

Append entries to `log` for notable progress, decisions, or surprises:

```json
{
  "timestamp": "2026-06-06T10:30:00Z",
  "author": "agent-name",
  "text": "Found that bumpalo doesn't support Drop; switched to typed-arena for nodes containing String."
}
```

If you hit a blocker, add an entry to `blockers` and set `status` to `"blocked"`.

### 4. Finish

When all checklist items are `done`:

- `status`: `"done"`
- `pr`: the PR URL or number
- `completed_at`: current ISO-8601 UTC timestamp
- Bump top-level `updated_at`

The PR that completes the work should include the `tasks.json` update.

## Conventions

- **One agent per task at a time.** Check `assignee` and `status` before claiming.
- **Don't edit other agents' tasks** unless you're rolling back an abandoned `in_progress` task (in which case, add a `log` entry explaining why).
- **Branch naming**: `<username>/rust-port-<task-id>-<short-desc>` — e.g., `jeremymeng/rust-port-m2-scanner`.
- **Checklist items are testable assertions**, not vague aspirations. If you can't write a test that proves an item is done, rewrite the item.
- **Append to `log`, never edit past entries.** Logs are the audit trail.
- **Update `tasks.json` in the same PR as the work**, not in a separate "tracking" PR.

## Querying common things

```bash
# What's currently in progress?
jq '.tasks[] | select(.status=="in_progress") | { id, title, assignee, branch }' tasks.json

# What's blocked and why?
jq '.tasks[] | select(.status=="blocked") | { id, blockers, log: (.log[-3:]) }' tasks.json

# Progress on a specific task
jq '.tasks[] | select(.id=="M2") | { id, status, checklist }' tasks.json

# Overall progress
jq '
  .tasks
  | group_by(.status)
  | map({ status: .[0].status, count: length })
' tasks.json
```

## When to update `plan.md`

`tasks.json` is the operational tracker — it changes constantly. `plan.md` is the design document — it changes only when decisions change.

Update `plan.md` when:

- A decision in the "Resolved Decisions" table changes (record old vs new in commit message)
- A cross-cutting constraint is added or removed
- A milestone is added, removed, or significantly re-scoped (also update `tasks.json` to match)
- M1.5 produces findings that change M3's API design

Do **not** update `plan.md` for routine progress — that goes in `tasks.json`'s `log`.
