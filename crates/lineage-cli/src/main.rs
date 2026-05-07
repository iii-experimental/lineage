//! lineage CLI shim.
//!
//! Each subcommand is one `iii.trigger` call against the local engine, except:
//! - `init`: writes `.claude/settings.json` hook block + checks engine reachability
//! - `agent list/add/remove`: shells to `iii worker {list,add,remove}`
//! - `hook <agent> <event>`: relays stdin → `POST /hook/<agent>/<event>` so any
//!   AI-coding runtime that can only fork-exec a shell command (not localhost
//!   HTTP) still works.
use clap::{Parser, Subcommand};
use iii_sdk::{III, InitOptions, TriggerRequest, register_worker};
use std::path::PathBuf;
use tokio::io::AsyncReadExt;

const DEFAULT_ENGINE_URL: &str = "ws://127.0.0.1:49234";
const DEFAULT_HTTP_BASE: &str = "http://127.0.0.1:3211";

#[derive(Parser, Debug)]
#[command(
    name = "lineage",
    version,
    about = "AI-session capture wired into git.",
    long_about = "lineage hooks any supported AI coding runtime, snapshots the working tree on a shadow git ref at every prompt, and ties each snapshot to a session-tree entry so the chain from prompt → file change → commit is recoverable."
)]
struct Cli {
    /// WebSocket URL of the local iii engine.
    #[arg(
        long,
        env = "III_URL",
        default_value = DEFAULT_ENGINE_URL,
        global = true
    )]
    engine_url: String,

    /// HTTP base URL for the engine's iii-http worker (used by `hook` relay).
    #[arg(
        long,
        env = "LINEAGE_HTTP_BASE",
        default_value = DEFAULT_HTTP_BASE,
        global = true
    )]
    http_base: String,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Initialise a project: write `.claude/settings.json` hook block, verify engine is reachable.
    Init {
        /// Project root. Defaults to current directory.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Overwrite an existing `.claude/settings.json` if present.
        #[arg(long)]
        force: bool,
    },
    /// Run preflight checks: engine reachable, ports, git repo, hook block, workers connected.
    Doctor {
        /// Project root to check. Defaults to current directory.
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Mark this project as enabled for capture.
    Enable,
    /// Disable capture for this project.
    Disable,
    /// Print engine + workers + repo + current-session state.
    Status,
    /// Manage captured sessions.
    Session {
        #[command(subcommand)]
        cmd: SessionCmd,
    },
    /// Manage shadow-ref checkpoints.
    Checkpoint {
        #[command(subcommand)]
        cmd: CheckpointCmd,
    },
    /// Search captured checkpoints (BM25, planned in v0.2).
    Search {
        /// Free-text query.
        query: String,
    },
    /// Manage the per-runtime hook-* worker adapters.
    Agent {
        #[command(subcommand)]
        cmd: AgentCmd,
    },
    /// Generate a markdown recap of recent agent work (planned in v0.2).
    Recap {
        /// Time window, e.g. `1d`, `1w`. Optional.
        #[arg(long)]
        since: Option<String>,
    },
    /// Relay a shell-fired runtime hook to lineage's HTTP endpoint.
    ///
    /// Reads the runtime's hook payload from stdin and POSTs it to
    /// `<http_base>/hook/<agent>/<event>`. Use as a fallback for runtimes
    /// that can only invoke a shell command, not localhost HTTP directly.
    Hook {
        /// Runtime name as it appears in the URL (claude-code, codex, gemini-cli, ...).
        agent: String,
        /// Event slug (stop, user-prompt-submit, pre-task, post-task, todo, session-start).
        event: String,
    },
}

#[derive(Subcommand, Debug)]
enum SessionCmd {
    /// List every captured session.
    List,
    /// Mark a session as the current one (subsequent hooks attach to it).
    Resume {
        /// Session id (full or prefix).
        id: String,
    },
    /// Show the active path of entries within a session.
    Attach {
        /// Session id.
        id: String,
    },
}

#[derive(Subcommand, Debug)]
enum CheckpointCmd {
    /// List every checkpoint across every session.
    List,
    /// Restore the working tree to a specific checkpoint.
    Rewind {
        /// Session id the entry belongs to.
        #[arg(long)]
        session: String,
        /// Entry id (12-hex-char checkpoint id).
        #[arg(long)]
        entry: String,
        /// Discard uncommitted local changes.
        #[arg(long)]
        force: bool,
    },
    /// Print a markdown summary for one checkpoint (planned in v0.2).
    Explain {
        /// Entry id.
        id: String,
    },
}

#[derive(Subcommand, Debug)]
enum AgentCmd {
    /// List installed `hook-*` workers via the iii worker registry.
    List,
    /// Install a `hook-*` worker (`iii worker add <pkg>`).
    Add {
        /// Worker name or OCI image reference.
        pkg: String,
    },
    /// Remove a `hook-*` worker.
    Remove {
        /// Worker name.
        name: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.cmd {
        Cmd::Init { path, force } => return cmd_init(path, force, &cli.http_base).await,
        Cmd::Doctor { path } => {
            return cmd_doctor(path, &cli.http_base, &cli.engine_url).await;
        }
        Cmd::Hook { agent, event } => return cmd_hook(&cli.http_base, &agent, &event).await,
        Cmd::Agent { cmd } => return cmd_agent(cmd).await,
        Cmd::Checkpoint {
            cmd: CheckpointCmd::Explain { id },
        } => {
            println!(
                "`lineage checkpoint explain {id}` is planned for v0.2 (lineage-recap worker)."
            );
            return Ok(());
        }
        Cmd::Search { query } => {
            println!(
                "`lineage search '{query}'` is planned for v0.2 (lineage-search worker, BM25 over checkpoints)."
            );
            return Ok(());
        }
        Cmd::Recap { since: _ } => {
            println!("`lineage recap` is planned for v0.2 (lineage-recap worker).");
            return Ok(());
        }
        _ => {}
    }

    // Remaining commands talk to the engine over WS.
    let iii = register_worker(&cli.engine_url, InitOptions::default());

    match cli.cmd {
        Cmd::Enable => trigger(&iii, "lineage::enable", &serde_json::json!({})).await?,
        Cmd::Disable => trigger(&iii, "lineage::disable", &serde_json::json!({})).await?,
        Cmd::Status => trigger(&iii, "lineage::status", &serde_json::json!({})).await?,
        Cmd::Session { cmd } => match cmd {
            SessionCmd::List => trigger(&iii, "session::list", &serde_json::json!({})).await?,
            SessionCmd::Resume { id } => {
                trigger(
                    &iii,
                    "lineage::resume",
                    &serde_json::json!({"session_id": id}),
                )
                .await?
            }
            SessionCmd::Attach { id } => {
                trigger(
                    &iii,
                    "session::active_path",
                    &serde_json::json!({"session_id": id}),
                )
                .await?
            }
        },
        Cmd::Checkpoint { cmd } => match cmd {
            CheckpointCmd::List => trigger(&iii, "session::tree", &serde_json::json!({})).await?,
            CheckpointCmd::Rewind {
                session,
                entry,
                force,
            } => {
                trigger(
                    &iii,
                    "lineage::rewind",
                    &serde_json::json!({"session_id": session, "entry_id": entry, "force": force}),
                )
                .await?
            }
            CheckpointCmd::Explain { .. } => unreachable!("handled above"),
        },
        // Already returned above:
        Cmd::Init { .. }
        | Cmd::Doctor { .. }
        | Cmd::Hook { .. }
        | Cmd::Agent { .. } => unreachable!(),
        Cmd::Search { .. } | Cmd::Recap { .. } => unreachable!(),
    }

    iii.shutdown_async().await;
    Ok(())
}

async fn trigger(iii: &III, function_id: &str, payload: &serde_json::Value) -> anyhow::Result<()> {
    let res = iii
        .trigger(TriggerRequest {
            function_id: function_id.to_string(),
            payload: payload.clone(),
            action: None,
            timeout_ms: Some(2_000),
        })
        .await?;
    println!("{}", serde_json::to_string_pretty(&res)?);
    Ok(())
}

// ---------- `lineage init` ----------

async fn cmd_init(path: Option<PathBuf>, force: bool, http_base: &str) -> anyhow::Result<()> {
    let root = match path {
        Some(p) => p,
        None => std::env::current_dir()?,
    };

    if !root.join(".git").exists() {
        anyhow::bail!(
            "{} is not a git repo. Run `git init` first; lineage requires git.",
            root.display()
        );
    }

    // Check engine reachability.
    let healthy = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(500))
        .build()?
        .get(format!("{http_base}/"))
        .send()
        .await
        .map(|r| r.status().as_u16() != 0)
        .unwrap_or(false);

    if !healthy {
        eprintln!(
            "warn: iii engine not reachable at {http_base}. Run `./scripts/dev.sh` (or your equivalent) before driving real hooks."
        );
    }

    // Write .claude/settings.json hook block.
    let claude_dir = root.join(".claude");
    let settings_path = claude_dir.join("settings.json");

    if settings_path.exists() && !force {
        eprintln!(
            "{} already exists. Re-run with --force to overwrite, or merge the hook block manually.",
            settings_path.display()
        );
    } else {
        std::fs::create_dir_all(&claude_dir)?;
        std::fs::write(&settings_path, claude_settings_block(http_base))?;
        println!("wrote {} (Claude Code hook block)", settings_path.display());
    }

    println!();
    println!("Next:");
    println!("  1. (one terminal)  ./scripts/dev.sh");
    println!("  2. (another)       claude    # or your AI coding runtime");
    println!("  3. (after a turn)  lineage checkpoint list");
    Ok(())
}

fn claude_settings_block(http_base: &str) -> String {
    let curl = |event: &str| {
        format!(
            "curl -sS -X POST {http_base}/hook/claude-code/{event} -H 'Content-Type: application/json' --data-binary @-"
        )
    };
    serde_json::to_string_pretty(&serde_json::json!({
        "hooks": {
            "SessionStart": [{"hooks": [{"type":"command","command": curl("session-start")}]}],
            "UserPromptSubmit": [{"hooks": [{"type":"command","command": curl("user-prompt-submit")}]}],
            "Stop": [{"hooks": [{"type":"command","command": curl("stop")}]}],
            "PreToolUse": [{"hooks": [{"type":"command","command": curl("pre-task")}]}],
            "PostToolUse": [{"hooks": [{"type":"command","command": curl("post-task")}]}],
        }
    }))
    .unwrap()
}

// ---------- `lineage agent {list,add,remove}` ----------

async fn cmd_agent(cmd: AgentCmd) -> anyhow::Result<()> {
    match cmd {
        AgentCmd::List => {
            // Filter `iii worker list` for hook-* names.
            let out = tokio::process::Command::new("iii")
                .args(["worker", "list"])
                .output()
                .await
                .map_err(|e| anyhow::anyhow!("iii worker list failed: {e}. Is `iii` installed?"))?;
            let stdout = String::from_utf8_lossy(&out.stdout);
            let mut found = 0;
            for line in stdout.lines() {
                if line.contains("hook-") {
                    println!("{line}");
                    found += 1;
                }
            }
            if found == 0 {
                println!("No `hook-*` workers installed via the iii registry.");
                println!(
                    "Note: lineage's bundled `hook-claude-code` and `hook-runtime-events` are run directly from this repo via `scripts/dev.sh` — they don't appear in `iii worker list` until they're published."
                );
            }
            Ok(())
        }
        AgentCmd::Add { pkg } => {
            let status = tokio::process::Command::new("iii")
                .args(["worker", "add", &pkg])
                .status()
                .await
                .map_err(|e| anyhow::anyhow!("iii worker add failed: {e}"))?;
            if !status.success() {
                anyhow::bail!("iii worker add {pkg} exited {status}");
            }
            Ok(())
        }
        AgentCmd::Remove { name } => {
            let status = tokio::process::Command::new("iii")
                .args(["worker", "remove", &name])
                .status()
                .await
                .map_err(|e| anyhow::anyhow!("iii worker remove failed: {e}"))?;
            if !status.success() {
                anyhow::bail!("iii worker remove {name} exited {status}");
            }
            Ok(())
        }
    }
}

// ---------- `lineage hook <agent> <event>` ----------

async fn cmd_hook(http_base: &str, agent: &str, event: &str) -> anyhow::Result<()> {
    let mut body = Vec::new();
    tokio::io::stdin().read_to_end(&mut body).await?;

    let url = format!("{http_base}/hook/{agent}/{event}");
    let res = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("POST {url}: {e}. Is the iii engine running?"))?;

    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    println!("{text}");
    if !status.is_success() {
        std::process::exit(status.as_u16() as i32 / 100);
    }
    Ok(())
}

// ---------- `lineage doctor` ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocStatus {
    Pass,
    Warn,
    Fail,
}

impl DocStatus {
    fn label(self) -> &'static str {
        match self {
            DocStatus::Pass => "PASS",
            DocStatus::Warn => "WARN",
            DocStatus::Fail => "FAIL",
        }
    }
}

struct DoctorRow {
    check: &'static str,
    status: DocStatus,
    detail: String,
}

async fn cmd_doctor(
    path: Option<PathBuf>,
    http_base: &str,
    engine_url: &str,
) -> anyhow::Result<()> {
    let root = path.unwrap_or(std::env::current_dir()?);
    let mut rows: Vec<DoctorRow> = Vec::new();

    // 1. HTTP base reachable.
    rows.push(check_http(http_base).await);

    // 2. WS engine reachable (TCP-connect to the parsed host:port).
    rows.push(check_ws_tcp(engine_url).await);

    // 3. Project is a git repo.
    rows.push(check_git(&root));

    // 4. .claude/settings.json exists + has lineage hooks.
    rows.push(check_hook_block(&root, http_base));

    // 5-7. Function registrations (only meaningful if WS is up).
    let ws_up = matches!(rows[1].status, DocStatus::Pass);
    if ws_up {
        let iii = register_worker(engine_url, InitOptions::default());
        for (label, fn_id, payload) in [
            (
                "lineage::status registered",
                "lineage::status",
                serde_json::json!({}),
            ),
            (
                "hook::claude_code::detect registered",
                "hook::claude_code::detect",
                serde_json::json!({"raw": {}}),
            ),
            (
                "hook::runtime_events::detect registered",
                "hook::runtime_events::detect",
                serde_json::json!({"raw": {}}),
            ),
        ] {
            rows.push(check_function(&iii, label, fn_id, payload).await);
        }
        iii.shutdown_async().await;
    } else {
        rows.push(DoctorRow {
            check: "lineage::status registered",
            status: DocStatus::Fail,
            detail: "skipped: engine not reachable".into(),
        });
        rows.push(DoctorRow {
            check: "hook::claude_code::detect registered",
            status: DocStatus::Fail,
            detail: "skipped: engine not reachable".into(),
        });
        rows.push(DoctorRow {
            check: "hook::runtime_events::detect registered",
            status: DocStatus::Fail,
            detail: "skipped: engine not reachable".into(),
        });
    }

    print_doctor(&rows);

    let fails = rows.iter().filter(|r| r.status == DocStatus::Fail).count();
    if fails > 0 {
        std::process::exit(1);
    }
    Ok(())
}

async fn check_http(http_base: &str) -> DoctorRow {
    let url = format!("{http_base}/");
    let result = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(500))
        .build()
        .and_then(|c| Ok(c.get(&url)));
    match result {
        Ok(req) => match req.send().await {
            // Any HTTP response means the engine's iii-http is alive.
            Ok(r) => DoctorRow {
                check: "HTTP base reachable",
                status: DocStatus::Pass,
                detail: format!("{} → {}", http_base, r.status().as_u16()),
            },
            Err(e) => DoctorRow {
                check: "HTTP base reachable",
                status: DocStatus::Fail,
                detail: format!("{http_base}: {e}. Run ./scripts/dev.sh."),
            },
        },
        Err(e) => DoctorRow {
            check: "HTTP base reachable",
            status: DocStatus::Fail,
            detail: format!("client build: {e}"),
        },
    }
}

async fn check_ws_tcp(engine_url: &str) -> DoctorRow {
    // Parse `ws://host:port`. iii-sdk doesn't expose a ping, so a TCP connect
    // is the cheapest way to know the engine is listening.
    let addr = engine_url
        .strip_prefix("ws://")
        .or_else(|| engine_url.strip_prefix("wss://"))
        .unwrap_or(engine_url);
    let host_port = addr.split('/').next().unwrap_or(addr);
    let connect = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        tokio::net::TcpStream::connect(host_port),
    )
    .await;
    match connect {
        Ok(Ok(_)) => DoctorRow {
            check: "WS engine reachable",
            status: DocStatus::Pass,
            detail: format!("{engine_url} TCP connect ok"),
        },
        Ok(Err(e)) => DoctorRow {
            check: "WS engine reachable",
            status: DocStatus::Fail,
            detail: format!("{engine_url}: {e}. Run ./scripts/dev.sh."),
        },
        Err(_) => DoctorRow {
            check: "WS engine reachable",
            status: DocStatus::Fail,
            detail: format!("{engine_url}: connect timed out"),
        },
    }
}

fn check_git(root: &std::path::Path) -> DoctorRow {
    if root.join(".git").exists() {
        DoctorRow {
            check: "Project is a git repo",
            status: DocStatus::Pass,
            detail: root.display().to_string(),
        }
    } else {
        DoctorRow {
            check: "Project is a git repo",
            status: DocStatus::Fail,
            detail: format!("{} has no .git/. Run `git init`.", root.display()),
        }
    }
}

fn check_hook_block(root: &std::path::Path, http_base: &str) -> DoctorRow {
    let settings = root.join(".claude").join("settings.json");
    if !settings.exists() {
        return DoctorRow {
            check: "Claude Code hook block present",
            status: DocStatus::Warn,
            detail: format!(
                "{} missing. Run `lineage init` to write it.",
                settings.display()
            ),
        };
    }
    let body = match std::fs::read_to_string(&settings) {
        Ok(s) => s,
        Err(e) => {
            return DoctorRow {
                check: "Claude Code hook block present",
                status: DocStatus::Fail,
                detail: format!("read {}: {e}", settings.display()),
            };
        }
    };
    let lineage_route = format!("{http_base}/hook/claude-code/");
    if body.contains(&lineage_route) {
        DoctorRow {
            check: "Claude Code hook block present",
            status: DocStatus::Pass,
            detail: format!("{} references {}", settings.display(), lineage_route),
        }
    } else {
        DoctorRow {
            check: "Claude Code hook block present",
            status: DocStatus::Warn,
            detail: format!(
                "{} exists but doesn't POST to {}. Run `lineage init --force`.",
                settings.display(),
                lineage_route
            ),
        }
    }
}

async fn check_function(
    iii: &III,
    label: &'static str,
    function_id: &str,
    payload: serde_json::Value,
) -> DoctorRow {
    let res = iii
        .trigger(TriggerRequest {
            function_id: function_id.to_string(),
            payload,
            action: None,
            timeout_ms: Some(800),
        })
        .await;
    match res {
        Ok(_) => DoctorRow {
            check: label,
            status: DocStatus::Pass,
            detail: function_id.to_string(),
        },
        Err(e) => DoctorRow {
            check: label,
            status: DocStatus::Fail,
            detail: format!("{function_id}: {e}. Worker not connected? Tail data/logs/<worker>.log."),
        },
    }
}

fn print_doctor(rows: &[DoctorRow]) {
    let max_check = rows.iter().map(|r| r.check.len()).max().unwrap_or(0);
    println!(
        "{:<width$}  STATUS  DETAILS",
        "CHECK",
        width = max_check.max(5)
    );
    println!(
        "{:-<width$}  ------  -------",
        "",
        width = max_check.max(5)
    );
    for row in rows {
        println!(
            "{:<width$}  {:<6}  {}",
            row.check,
            row.status.label(),
            row.detail,
            width = max_check.max(5)
        );
    }
    let pass = rows.iter().filter(|r| r.status == DocStatus::Pass).count();
    let warn = rows.iter().filter(|r| r.status == DocStatus::Warn).count();
    let fail = rows.iter().filter(|r| r.status == DocStatus::Fail).count();
    println!();
    println!("Summary: {pass} PASS, {warn} WARN, {fail} FAIL.");
}
