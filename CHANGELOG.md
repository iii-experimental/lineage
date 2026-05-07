# Changelog

All notable changes are recorded here. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), versioning: [SemVer](https://semver.org/spec/v2.0.0.html). lineage stays in 0.x until production usage proves out behaviour against live infra.

## [Unreleased]

## [0.2.0] - 2026-05-07

### Added
- `lineage doctor` — preflight that checks the seven things that actually break a fresh install: HTTP base reachable, WS engine TCP reachable, `--path` is a git repo, `.claude/settings.json` hook block present, `lineage::status` registered, both `hook::*::detect` functions registered. Prints an aligned `CHECK / STATUS / DETAILS` table with a `Summary: N PASS, N WARN, N FAIL` footer. WARN for "missing hook block" (recoverable via `lineage init`), FAIL only for "engine not running / function not registered". Verified against a clean engine: 7 PASS / 0 WARN / 0 FAIL on an initialized repo, 6 PASS / 1 WARN / 0 FAIL on a fresh `git init` with no settings.json.
- `lineage doctor --smoke` — after preflight, fires a synthetic Claude Code `Stop` hook against a temp git repo (with `cwd` carrying the temp path so multi-tenant routing kicks in), polls up to 3s for a shadow ref under `refs/iii/lineage/checkpoints/v0/<smoke-session>/`. Catches the "hook returned 200 but no checkpoint landed" runtime symptom (queue stuck, gitops crashed, ignore patterns swallowing the world) that doctor's preflight cannot detect.
- Multi-tenant `repo_path` from hook payload `cwd`. `NormalisedEvent` now carries `cwd: Option<String>`; both `hook-claude-code` and `hook-runtime-events` lift it from the runtime payload (`cwd` for Claude Code; `cwd` or `repo_path` for the generic shape). Strategy uses `evt.cwd.unwrap_or_else(|| ctx.repo_path())` so a single engine can capture sessions for multiple repos concurrently. Five new tests.
- `lineage::enable` / `lineage::disable` actually flip a real flag now. `StrategyContext.enabled: AtomicBool` defaults to `true`. While disabled, `handle_hook` short-circuits and returns `{blocked: true, message: "lineage capture is disabled..."}` without writing a session-tree entry, enqueueing a snapshot, or publishing to `agent::*` topics. Runtimes stay responsive (HTTP 200), nothing lands. v0.2 limitation: in-memory only, resets to `enabled` on engine restart.
- "Next:" hint after `lineage status` JSON, pointing at the iii-console URL, the end-to-end demo script, and `lineage init`. Closes the "is it working / what now?" gap immediately after install.
- Pause / resume capture section in README documents the kill-switch semantics.
- iii-console (`http://127.0.0.1:3213`) URL promoted in the install-flow section so first-time users discover live activity in the right minute.

### Changed
- `recap`, `search`, and `checkpoint explain` clap subcommands are now hidden from `--help`. They still resolve and print "planned in v0.2" stubs but don't pollute discovery while incomplete. They reappear in `--help` when the underlying workers ship.
- Default test count in README updated 32 → 37 (+ enable/disable + smoke covered by integration walk).
- `iii worker add` failure messages now append the "iii-hq workers are optional in v0.1+" note so devs don't assume lineage is broken when an external worker doesn't exist.
- `lineage::resolve_blob` "path component not found" error appends a fix hint pointing at `git ls-tree -r <tree>` and `git for-each-ref refs/iii/lineage/checkpoints/v0/`.

### Upgrading from v0.1.x

No wire-level breaking changes. v0.2.0-dev is fully additive over v0.1.0:

- `.claude/settings.json` unchanged. Existing hook blocks stay valid; no need to re-run `lineage init`.
- Existing shadow refs under `refs/iii/lineage/checkpoints/v0/` remain valid. Rewind, list, and resolve_blob still work against v0.1-era refs.
- `session-tree` entry shape unchanged.
- HTTP hook payload contract unchanged. Adding `cwd` to the runtime payload is **opt-in**: hooks that don't include it fall back to `LINEAGE_REPO_PATH` exactly like v0.1.
- `NormalisedEvent.cwd` field is internal to lineage workers and not exposed on any wire surface.

To upgrade: `git pull && cargo build --release`. That's it. Restart `dev.sh`. Existing sessions resume cleanly.

## [0.1.0] - 2026-05-06

First public 0.x release. Greenfield. Targets `iii-engine` 0.11.6+. Local-only (team aggregation via `iii-bridge`; iii cloud one-click deploy when it ships).

### Added
- HTTP hook ingest at `POST /hook/<agent>/<event>` (six lifecycle events).
- Pure-`gix` shadow-ref snapshot on `refs/iii/lineage/checkpoints/v0/<session>/<entry>`. Captures **untracked files**. Honors `.gitignore`.
- Rewind that restores untracked files alongside tracked.
- Commit-trailer attachment (`Lineage-Session`, `Lineage-Path`).
- Per-payload-shape hook normalisers: `hook-claude-code` (Claude Code's `hook_event_name` shape) + `hook-runtime-events` (the `{event, session_id, ...}` shape used by Codex / Gemini CLI / OpenCode / Cursor / Copilot CLI / Droid / future runtimes).
- `hook-fanout`-based `detect` routing (with a payload-shape fallback when fanout isn't on the bus).
- FIFO `lineage_checkpoint` queue with `message_group_field=session_id`. Concurrent hooks for the same session serialise.
- `lineage_worker_main!` macro consolidates worker boot to ~5 lines per binary while keeping `iii_sdk::register_worker(...)` calls textually visible per `cargo expand` (the "iii primitives first" rule).
- `lineage` CLI: `init`, `enable`, `disable`, `status`, `session list/resume/attach`, `checkpoint list/rewind/explain`, `search`, `agent list/add/remove`, `recap`, `hook` (real shell→HTTP relay).
- `lineage init` writes `.claude/settings.json` hook block + verifies engine reachability.
- `lineage hook <agent> <event>` reads stdin and POSTs to `<http_base>/hook/<agent>/<event>`. Fallback for runtimes that can only fork-exec.
- 32 unit tests (gitops + handlers + normalisers).
- `dev.sh` supervises lineage workers with restart-on-failure (bash 3.2 compatible — works on stock macOS).
- End-to-end smoke verified live: 12.3 ms mean per hook, 51/51 shadow refs landed.
- `examples/end-to-end-demo.sh` reproduces the live smoke locally.

### Engineering decisions
- `parking_lot::Mutex` (no panic poisoning) for `StrategyContext`.
- `route_request` bubbles `Err` through the engine pipeline; no hand-crafted 500 envelopes (so iii's OTel error spans + dead-letter routing fire correctly).
- `session::create` cached once per session in `StrategyContext`. Saves one round-trip per subsequent hook.
- Strategy publishes to `agent::after_tool_call` / `agent::before_tool_call` / `agent::events` topics so any future iii-hq subscriber (redaction, audit log, context compaction) attaches without lineage code changes.
- iii-hq workers (`session-tree`, `dlp-scrubber`, `audit-log`, `context-compaction`, `provider-router`) are **not required**. Lineage degrades gracefully: `entry_id` stays empty, shadow refs still land. Once those workers publish to the iii registry, `iii worker add <name>` plugs them in.

### Known gaps (tracked in README TODOS)
- `lineage init` does not yet detect non-Claude-Code runtimes; per-runtime settings paths land in v0.2.
- `lineage doctor` not built. Use README troubleshooting + `data/logs/<worker>.log`.
- `lineage recap` and `lineage search` are placeholders; real workers ship in v0.2.
- Demo GIF placeholder. Cold-traffic conversion gated on a real recording.
- Prebuilt release binaries (Homebrew tap, GitHub Release tarballs) ship in v0.2; v0.1 is `git clone && cargo build --release`.

### Performance
- Hook latency: 12.3 ms mean / 13 ms p50 (50-hook benchmark, single-replica engine, macOS).
- Snapshot latency dominated by gix tree walk + blob hashing of new/changed files; index reuse is a v0.2 optimisation.
- `lineage-cli` per-call cost: ~1.7 s due to full `register_worker` connection setup. Lighter client lands in v0.2.

[Unreleased]: https://github.com/iii-experimental/lineage/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/iii-experimental/lineage/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/iii-experimental/lineage/releases/tag/v0.1.0
