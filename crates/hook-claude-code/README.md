# hook-claude-code

Per-runtime normaliser for Claude Code hook payloads. Translates the `session-start`, `user-prompt-submit`, `stop`, `pre-task`, `post-task`, `todo` events into a `NormalisedEvent` that `lineage-strategy` can route through the rest of the iii-hq topology.

This crate is general-purpose — any iii roster consuming Claude Code runtime events (autoharness, agentmemory, future workers) can import it.

## Registered functions

| Function | Description |
|---|---|
| `hook::claude_code::normalise` | `payload` → `NormalisedEvent` |
| `hook::claude_code::detect` | `()` → `{ present: bool }` |
| `hook::claude_code::session_dir` | `{ repo_path }` → `{ session_dir }` |
| `hook::claude_code::read_session` | `{ session_dir, session_id }` → `Vec<TranscriptEntry>` |

## Status

0.1.0 stub. Phase 2 implementation pending.
