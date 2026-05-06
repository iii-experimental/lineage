# hook-runtime-events

Generic runtime hook normaliser. One crate, six runtimes covered.

The hook payloads emitted by Codex, Gemini CLI, OpenCode, Cursor, GitHub Copilot CLI, and Factory.ai Droid all share a common shape:

```
{ event, session_id, prompt?, call_id|tool_use_id?, tool|tool_name?, input|tool_input?, output|tool_result?, transcript_path? }
```

This crate translates that shape into the lineage `NormalisedEvent`. The decomposition axis is **per payload shape**, not per agent — every runtime adopting this shape needs zero new code.

Claude Code uses a different vocabulary (`hook_event_name`, `tool_response`); see `hook-claude-code`.

## Registered functions

| Function | Description |
|---|---|
| `hook::runtime_events::normalise` | `{agent, raw}` → `NormalisedEvent`. Caller passes the agent label since the payload doesn't carry one. |
| `hook::runtime_events::detect` | `{raw}` → `{match: bool, shape: "runtime_events"}`. Returns `match: true` when the payload has `event` and not `hook_event_name`. |

## Status

0.1.0. 6 unit tests covering event-name aliases, input/output lifting, shape detection.
