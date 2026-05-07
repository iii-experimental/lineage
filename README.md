# lineage

**Capture every AI agent coding session in git. Rewind any prompt.**

<!-- TODO: replace with a real GIF. ~10 seconds. Frames:
     1. Terminal: a Claude Code session edits `auth.rs`. File visible in editor.
     2. Agent runs again, "rewrites auth flow" — file changes wholesale.
     3. Run `lineage checkpoint list` — shows two entries with timestamps.
     4. Run `lineage rewind <first-id>` — `auth.rs` is back to the first state.
     5. Caption: "every prompt → snapshot → rewindable".
     Record with asciinema or Kap. Place at `docs/demo.gif`.
-->
<p align="center">
  <em>(demo GIF — coming, see TODOS)</em>
</p>

## Why this exists

You've shipped code with Claude Code, Codex, Gemini CLI, OpenCode, Cursor, Copilot CLI, or Droid. You've also:

- Watched an agent eat three files at prompt 7 and rummaged through `git reflog` to find them.
- Tried to explain in a PR review *why* a weird try/catch exists, with nothing to point at except your memory.
- Wanted to fork a session mid-conversation and try a different approach without losing the first.

`lineage` solves this. Every agent prompt → snapshot of your working tree onto a shadow git ref. Every commit → trailer pointing back to the session that produced it. Rewind to any point. `git log --show-trailers` exposes the chain from prompt → file change → commit.

## Status

**0.1.0** shipped 2026-05-06. **0.2.0-dev** in progress. Targets `iii-engine` 0.11.6+. Local-only for v0; team aggregation via `iii-bridge` (already supported by the engine, no extra code).

What works today: HTTP hook ingest, payload-shape-routed normalisers (Claude Code + a generic shape covering Codex / Gemini CLI / OpenCode / Cursor / Copilot CLI / Droid / any future runtime that emits `{event, session_id, ...}`), pure-`gix` shadow-ref snapshot capturing untracked files, rewind, list, blob resolve, commit-trailer attachment, CLI shim with `init` / `doctor` / `status` / `hook` / `checkpoint`, multi-tenant `repo_path` lift from hook payload `cwd`, `lineage_checkpoint` FIFO queue serialising concurrent snapshots per session, hook-fanout-based `detect` routing, `dev.sh`-supervised worker auto-restart. 37 unit tests; end-to-end smoke verified live; 12 ms mean / 13 ms p50 per hook.

What's planned (see [TODOS](#todos)): a real GIF, `lineage recap` (LLM session summaries), `lineage search` (BM25 over checkpoints), prebuilt release binaries.

## 60-second install

> Requires Rust 1.85+, git, and free `:49234` / `:3211` / `:3212` / `:3213` on localhost.

```bash
# 1. iii engine (~10s)
curl -fsSL https://install.iii.dev/iii/main/install.sh | sh

# 2. clone + build (~50s first run, cached after)
git clone https://github.com/iii-experimental/lineage
cd lineage
cargo build --release

# 3. boot engine + 4 lineage workers (point at the project you want to capture)
LINEAGE_REPO_PATH="$(pwd)" ./scripts/dev.sh
```

Leave that terminal running. In another shell:

```bash
./target/release/lineage status
```

You should see:

```json
{
  "current_session_id": null,
  "enabled": false,
  "engine_url": "ws://127.0.0.1:49234",
  "http_base": "http://127.0.0.1:3211",
  "repo_path": "/path/to/your/project",
  "console_hint": "open http://127.0.0.1:3213 for live worker / function / queue state"
}
```

If the JSON prints, lineage is healthy.

> **iii-hq workers are optional.** Lineage runs standalone in v0.1. Once `session-tree`, `hook-fanout`, `dlp-scrubber`, `audit-log`, `context-compaction`, and `provider-router` publish to the iii worker registry, `iii worker add <name>` plugs them in for free. Until then lineage degrades gracefully: `entry_id` stays empty, shadow refs still land.

## First checkpoint walk-through

Run [`examples/end-to-end-demo.sh`](./examples/end-to-end-demo.sh). It scaffolds a throwaway git repo with one untracked file, fires a fake Claude Code `Stop` hook, verifies the shadow ref landed in git, deletes the file, runs rewind, and asserts the file is back. Smoke-passes in ~3 seconds.

```bash
./examples/end-to-end-demo.sh
```

Or step through manually. From a separate terminal (with `dev.sh` running in another):

```bash
# fresh repo for the demo
SMOKE=/tmp/lineage-smoke
rm -rf "$SMOKE" && mkdir -p "$SMOKE" && cd "$SMOKE"
git init -q -b main
git config user.email demo@example.com && git config user.name demo
echo "hello world" > README.md
git add . && git commit -qm "init"
echo "secret-untracked content" > brand_new.txt   # <-- the file an agent just created

# fire a fake Claude Code Stop hook (lineage-strategy is already pointed at $SMOKE
# via LINEAGE_REPO_PATH=$SMOKE in the dev.sh terminal)
curl -sS -X POST http://127.0.0.1:3211/hook/claude-code/stop \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"demo-1","hook_event_name":"Stop"}'
```

Expected response (the `shadow_ref` field is a queue receipt; the real git ref lands a moment later when `lineage-gitops` consumes the queue):

```json
{
  "session_id": "demo-1",
  "entry_id": "",
  "shadow_ref": "queued:787ef7ed-4fa8-4773-a175-83027279f229",
  "blocked": false,
  "message": null
}
```

> `entry_id` is empty until the `session-tree` worker is on the bus (it's optional in v0.1; see the box at the top of the install). The shadow ref still lands regardless.

A second later, the real shadow ref is in git:

```bash
git -C "$SMOKE" for-each-ref refs/iii/lineage/checkpoints/v0/
# <commit-oid> commit  refs/iii/lineage/checkpoints/v0/demo-1/<12hex>
```

That ref is a full snapshot of `$SMOKE`'s working tree at the moment the hook fired — **including untracked files** (e.g., a new `auth.rs` an agent just wrote). It travels with the repo (`git push --mirror` ships it), survives `git reset --hard` because shadow refs aren't pruned, and honors `.gitignore` so `secret.env` won't accidentally enter the tree.

## Rewind demo

```bash
# break the working tree
echo "DESTROYED" > "$SMOKE/README.md"
cat "$SMOKE/README.md"   # DESTROYED

# read the shadow ref from above
REF=$(git -C "$SMOKE" for-each-ref refs/iii/lineage/checkpoints/v0/demo-1/ \
  --format='%(refname)' | head -1)

# rewind via lineage
iii trigger --port 49234 --function-id lineage::rewind_to \
  --payload "{\"repo_path\":\"$SMOKE\",\"shadow_ref\":\"$REF\",\"force\":true}"

cat "$SMOKE/README.md"   # hello world  -- the agent's edit is gone, original restored
```

That's the loop: prompt → snapshot → rewind. Wire your real Claude Code into it next.

## Wiring Claude Code (and friends)

Each AI coding runtime fires shell hooks at lifecycle points. Lineage exposes one HTTP endpoint per `(agent, event)` pair: `POST http://127.0.0.1:3211/hook/<agent>/<event>`. Configure your runtime to POST its hook payload there.

The fast path:

```bash
cd /your/project
lineage init
```

That writes the Claude Code hook block to `.claude/settings.json` and verifies the engine is reachable. Sanity-check with:

```bash
lineage doctor                  # current directory
lineage doctor --path /repo     # explicit
```

`doctor` runs 7 preflight checks (HTTP base, WS engine, git repo, hook block, three function registrations) and prints an aligned table. Returns non-zero if any FAIL, exits 0 on PASS or WARN.

If you'd rather paste the hook block manually, drop this in:

```json
{
  "hooks": {
    "SessionStart":      [{"hooks": [{"type": "command", "command": "curl -sS -X POST http://127.0.0.1:3211/hook/claude-code/session-start -H 'Content-Type: application/json' --data-binary @-"}]}],
    "UserPromptSubmit":  [{"hooks": [{"type": "command", "command": "curl -sS -X POST http://127.0.0.1:3211/hook/claude-code/user-prompt-submit -H 'Content-Type: application/json' --data-binary @-"}]}],
    "Stop":              [{"hooks": [{"type": "command", "command": "curl -sS -X POST http://127.0.0.1:3211/hook/claude-code/stop -H 'Content-Type: application/json' --data-binary @-"}]}],
    "PreToolUse":        [{"hooks": [{"type": "command", "command": "curl -sS -X POST http://127.0.0.1:3211/hook/claude-code/pre-task -H 'Content-Type: application/json' --data-binary @-"}]}],
    "PostToolUse":       [{"hooks": [{"type": "command", "command": "curl -sS -X POST http://127.0.0.1:3211/hook/claude-code/post-task -H 'Content-Type: application/json' --data-binary @-"}]}]
  }
}
```

Other runtimes follow the same pattern — agent name in the URL, event name as path. lineage detects the payload shape via `hook-fanout` and routes to the right normaliser: Claude Code uses `hook-claude-code` (its own vocabulary, e.g. `hook_event_name`, `tool_response`); Codex / Gemini CLI / OpenCode / Cursor / Copilot CLI / Droid all share the generic `{event, session_id, ...}` shape and route to `hook-runtime-events`. Adding a runtime that adopts the generic shape needs zero new code.

After running `lineage init`, restart Claude Code (or your runtime) so it picks up the new settings.

## Inspect what was captured

```bash
# every shadow ref in this repo
iii trigger --port 49234 --function-id lineage::list_shadow_refs \
  --payload '{"repo_path":"/your/project"}'

# read a captured file from a specific checkpoint
iii trigger --port 49234 --function-id lineage::resolve_blob \
  --payload '{"repo_path":"/your/project","shadow_ref":"<ref>","path":"src/auth.rs"}'

# attach commit trailers tying a real commit to its session
iii trigger --port 49234 --function-id lineage::attach_trailers \
  --payload '{"repo_path":"/your/project","commit_oid":"<oid>","session_id":"<sid>","entry_path":["<entry-id>"]}'
```

The live `iii-console` at `http://127.0.0.1:3213` renders the same data: registered functions, traces, queues, dead-letter, state slices, real-time stream activity. No custom UI in this repo — composes the stock console.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `curl: Failed to connect to 127.0.0.1 port 3211` | iii engine not running | Run `./scripts/dev.sh` in another shell. |
| Engine boot panics: `address already in use` on `:3112` or `:49134` | another iii engine already running on default ports | Lineage uses `:3211 / :3212 / :49234` — make sure no other process is on those. Override with `HTTP_PORT`, `STREAM_PORT`, `WORKER_PORT` env vars. |
| Hook returns 200 but `entry_id: ""` | `session-tree` worker not on the bus | Optional in v0.1. Shadow refs still land. When `session-tree` ships to the iii registry, `iii worker add session-tree` plugs it in. |
| `shadow_snapshot` errors with `not a git repository` | `LINEAGE_REPO_PATH` points outside a git repo | Set `LINEAGE_REPO_PATH=/path/to/your/git/project` before launching `dev.sh`, or `git init` the directory. |
| Hook fires but no checkpoint lands | `.claude/settings.json` not configured | Run `lineage init` from your project root. Restart Claude Code. |
| `cargo build --release` takes ~50 s first run | Compiling iii-sdk + tokio + gix from scratch | One-time. Subsequent builds are seconds. Prebuilt binary releases planned (see TODOS). |
| Hook returns 200 but `shadow_ref` looks like `queued:<uuid>` | Intentional. Snapshot is on the FIFO `lineage_checkpoint` queue; `lineage-gitops` consumes a moment later. | Real ref via `git for-each-ref refs/iii/lineage/checkpoints/v0/<session_id>/` or `lineage checkpoint list`. |
| Worker dies and never comes back | `dev.sh` supervisor failed | Tail `data/logs/<worker>.log` for the panic; restart is automatic within ~2 s. If restarts loop, the worker has a real bug. |
| `lineage status` hangs, then errors `invocation timed out` | Default port mismatch with a custom engine deploy | Pass `--engine-url ws://host:port` or set `III_URL`. Default is `ws://127.0.0.1:49234` (matches `dev.sh`). |
| `iii worker add <name>` says `Worker '<name>' not found` | iii-hq registry doesn't have that worker yet | Lineage doesn't require any iii-hq workers in v0.1. Skip the step; lineage runs standalone. |

If none of these match, run with `RUST_LOG=debug` and open an issue with the trailing logs.

## Architecture

<details>
<summary>Click to expand — workers, topics, file layout</summary>

### Workers in this repo

| Crate | Role |
|---|---|
| `lineage-strategy` | The brain. Owns one HTTP trigger (`POST /hook/:agent/:event`). Detects payload shape via `hook-fanout::publish_collect`, dispatches to the matching normaliser, writes a session-tree entry, **enqueues** the shadow snapshot through the FIFO `lineage_checkpoint` queue (one snapshot at a time per session), publishes to `agent::*` topics so existing iii-hq subscribers attach. |
| `lineage-gitops` | Pure-`gix` shadow-ref snapshot (captures untracked files, honors `.gitignore`), rewind that restores untracked files too, commit-trailer attachment. Subscribes to the `lineage_checkpoint` queue. |
| `lineage-cli` | Thin shim binary. Each subcommand is one `iii.trigger` call. |
| `lineage-worker-macro` | Declarative `lineage_worker_main!` macro. Expands inline so each worker's `main.rs` is ~5 lines and `cargo expand` shows direct `iii_sdk::register_worker(...)` calls per the "iii primitives first" rule. |
| `hook-claude-code` | Per-runtime normaliser for Claude Code's payload shape (`hook_event_name`, `tool_response`, `PostToolUse + tool_name=TodoWrite → todo` branch). Registers `hook::claude_code::detect` + `normalise`. |
| `hook-runtime-events` | Generic normaliser for the common runtime payload shape (`{event, session_id, prompt?, tool_*?, input/output}`). Covers Codex, Gemini CLI, OpenCode, Cursor, GitHub Copilot CLI, Factory.ai Droid, and any future runtime that adopts the shape. Registers `hook::runtime_events::detect` + `normalise`. |

### Optional workers (v0.2+, plug-and-play once published to iii registry)

None of these are required for v0.1 — lineage publishes the right `agent::*` topics and calls the right function ids opportunistically. When the iii-hq registry ships any of these, `iii worker add <name>` plugs them in with no lineage code change.

| Concern | Worker | Source |
|---|---|---|
| Checkpoint storage | `session-tree` | iii-hq/workers (planned) |
| Redaction | `dlp-scrubber` | iii-hq/workers (planned) |
| Audit trail | `audit-log` | iii-hq/workers (planned) |
| Context compaction | `context-compaction` | iii-hq/workers (planned) |
| Hook fan-out | `hook-fanout` | iii-hq/workers (planned) |
| Per-tool guardrails | `policy-denylist`, `guardrails` | iii-hq/workers (planned) |
| Corpus export | `session-corpus` | iii-hq/workers (planned) |
| Summarisation | `provider-router` + `provider-anthropic` / etc. | iii-hq/workers (planned) |
| LLM cost caps | `llm-budget` | iii-hq/workers (planned) |
| Provider auth | `auth-credentials`, `oauth-anthropic`, `oauth-openai-codex`, ... | iii-hq/workers (planned) |
| MCP exposure | `mcp` | iii-hq/workers (planned) |
| Live UI | stock `iii-console` (`:3213`) | iii-engine (works today) |
| Team aggregation | `iii-bridge` | iii-engine (works today) |
| Observability | `iii-observability` (OTel) | iii-engine (works today) |

### Data flow per hook event

```
Claude Code (or any runtime)
        │  POST /hook/claude-code/stop
        ▼
iii-http :3211
        │
        ▼
lineage-strategy:lineage::handle_hook_route
        │  iii.trigger("hooks::publish_collect",
        │     {topic: "lineage::detect", payload: {raw}})  (hook-fanout)
        ▼                                                     │
hook-{claude_code,runtime_events}::detect → {match, shape} ◄──┘
        │
        │  iii.trigger("hook::<shape>::normalise", raw)
        ▼                                              │
NormalisedEvent ◄──────────────────────────────────────┘
        │
        │  iii.trigger("session::append", entry)              (session-tree)
        │  iii.trigger(                                       (FIFO queue,
        │     "lineage::shadow_snapshot", payload,             one snapshot
        │     action=Enqueue{queue: "lineage_checkpoint"})     at a time per
        │                                                      session_id)
        │           │
        │           ▼  (consumed by lineage-gitops)
        │       gix builds tree → writes commit → updates
        │       refs/iii/lineage/checkpoints/v0/<sid>/<eid>
        │
        │  iii.trigger("iii::durable::publish",
        │       {topic: "agent::after_tool_call", ...})
        ▼
{session_id, entry_id, shadow_ref: "queued:<receipt-id>"} → caller
```

The `shadow_ref` field returned to the HTTP caller is a queue receipt id (e.g. `queued:787e...`). The actual shadow git ref lands a moment later when `lineage-gitops` consumes the queue. Look it up via `git for-each-ref refs/iii/lineage/checkpoints/v0/<session_id>/` or `iii.trigger("lineage::list_shadow_refs", ...)`.

Subscribers on `agent::after_tool_call` (`dlp-scrubber`, `audit-log`, `context-compaction`) attach automatically — lineage publishes the topic, doesn't have to integrate them per-worker.

### Shadow-ref layout

```
refs/iii/lineage/checkpoints/v0/<session_id>/<entry_id>
                                  │            │
                                  │            └─ random 12-hex chars
                                  └─ session id, e.g. 2026-05-06-abc123
```

Each ref is a real commit object pointing at a real tree, built natively via `gix` from the working directory (including untracked files, honoring `.gitignore`). Refs travel with `git push --mirror`. They survive `git gc` because every ref is reachable.

### Repo layout

```
lineage/
├── Cargo.toml                       # workspace, edition 2024, rust-version 1.85
├── crates/
│   ├── lineage-strategy/            # the brain (HTTP triggers + dispatch)
│   ├── lineage-gitops/              # pure-gix shadow refs + rewind + trailers
│   ├── lineage-cli/                 # `lineage` shim binary
│   ├── lineage-worker-macro/        # `lineage_worker_main!` macro
│   ├── hook-claude-code/            # Claude-Code-specific normaliser
│   └── hook-runtime-events/         # generic normaliser (Codex/Gemini/OpenCode/Cursor/Copilot/Droid/...)
├── iii.config.yaml                  # engine config: builtins on :3211/:3212/:49234
├── scripts/dev.sh                   # boots engine + 4 workers, supervises restart
├── README.md                        # this file
└── LICENSE-APACHE
```

</details>

## Development

```bash
cargo test --workspace          # 37 tests, includes gitops tempdir-git tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo build --workspace --release
```

CI runs all four on every push and PR (see `.github/workflows/ci.yml`).

## TODOS

Tracked in this repo for v0.2.0:

- [ ] Real demo GIF at `docs/demo.gif`. ~10s. The five frames described at the top of this file.
- [ ] Prebuilt release binaries via GitHub Release (cuts the 50s `cargo build` from install).
- [ ] `lineage recap` worker — composes session-tree entries + provider-router into markdown summaries.
- [ ] `lineage search` worker — BM25 over checkpoint entries via stream subscription.
- [ ] Native `iii-worker-manager` supervision once iii's worker registry supports local-dev binary registration (today only registry/OCI workers). Until then `dev.sh` does the supervising.
- [ ] Promote `hook-claude-code` and `hook-runtime-events` to `iii-hq/workers` once dogfooded (smallest README-touch first per multi-PR merge order).

Done since v0.1.0:

- [x] `lineage doctor` — 7-check preflight, aligned table, WARN for recoverable / FAIL for engine-down. Verified live: 7 PASS on initialized repo, 6 PASS / 1 WARN on `git init` without settings.json.
- [x] Multi-tenant `repo_path` from hook payload `cwd`. One engine captures sessions for multiple repos concurrently. Both normalisers lift `cwd` (and `repo_path` for the generic shape).

Done in v0.1.0:

- [x] `lineage init <project-path>` — writes `.claude/settings.json` hook block + verifies engine reachability.
- [x] Pure-`gix` shadow-snapshot path. No fork-execs. Captures untracked files. Honors `.gitignore`. 13 ms p50 (down from 91 ms).
- [x] Collapsed 7 sibling normaliser crates into 1 generic + 1 Claude-Code-specific (~1,200 LOC delete).
- [x] FIFO queue serialising concurrent snapshots per session.
- [x] hook-fanout-based payload-shape detection.
- [x] `lineage_worker_main!` macro (each `main.rs` is ~5 lines; iii primitives stay textually visible per `cargo expand`).
- [x] Errors bubble through engine pipeline (not hand-crafted 500 envelope).
- [x] `parking_lot::Mutex` (no panic poisoning).
- [x] `session::create` cached once-per-session (one extra round-trip, not per-hook).
- [x] `dev.sh` supervises lineage workers with restart-on-failure (bash 3.2 compatible).

## License

Apache-2.0. See [LICENSE-APACHE](./LICENSE-APACHE).
