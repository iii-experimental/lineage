//! `lineage_worker_main!` — declarative macro for worker boot.
//!
//! Expands inline at compile time. `cargo expand` shows direct
//! `iii_sdk::register_worker(...)` + `iii.shutdown_async()` calls in each
//! worker's main.rs. No abstraction tier, no hidden primitive.
//!
//! Why a macro instead of a `run_worker(...)` function: lineage's design rule
//! is "iii primitives first" — every worker must visibly call iii-sdk. A
//! function call would hide those calls behind one symbol; the macro keeps
//! them textually present. Same DRY win, no rule violation.
//!
//! ## Usage
//!
//! ```ignore
//! lineage_worker_macro::lineage_worker_main!(
//!     "hook-claude-code",
//!     hook_claude_code::registration::register,
//! );
//! ```
//!
//! Equivalent to:
//!
//! ```ignore
//! fn main() -> anyhow::Result<()> {
//!     // ... ~25 lines of tokio runtime + tracing init + register_worker + ctrl_c
//! }
//! ```

#[macro_export]
macro_rules! lineage_worker_main {
    ($name:expr, $register:path) => {
        fn main() -> ::anyhow::Result<()> {
            use ::clap::Parser as _;
            use ::iii_sdk::{InitOptions, register_worker};

            ::tracing_subscriber::fmt()
                .with_env_filter(
                    ::tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| ::tracing_subscriber::EnvFilter::new("info")),
                )
                .json()
                .init();

            #[derive(::clap::Parser, Debug)]
            #[command(name = $name, version)]
            struct __WorkerArgs {
                #[arg(long, env = "III_URL", default_value = "ws://127.0.0.1:49134")]
                engine_url: String,
                #[arg(long, env = "III_CONFIG")]
                config: Option<String>,
            }

            let args = __WorkerArgs::parse();
            let runtime = ::tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                let iii = register_worker(&args.engine_url, InitOptions::default());
                $register(&iii);
                ::tracing::info!(
                    engine_url = %args.engine_url,
                    worker = $name,
                    "lineage worker connected"
                );
                ::tokio::signal::ctrl_c().await?;
                iii.shutdown_async().await;
                Ok::<(), ::anyhow::Error>(())
            })
        }
    };
}
