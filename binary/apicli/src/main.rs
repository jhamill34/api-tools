//! The CLI binary: a thin gRPC client to `apid`, plus local scaffolding
//! tools for generating new service definitions and an embedded
//! interactive-login web server.

mod commands;
mod config;
mod constants;
mod engine;
mod io_util;
mod path;
mod schema;
mod stub;
mod template;

use clap::Parser;
use dotenv::dotenv;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();

    #[cfg(feature = "dhat-ad-hoc")]
    let _profiler = dhat::Profiler::new_ad_hoc();

    dotenv().ok();
    let cli = commands::Cli::parse();
    let mut engine = engine::Cli::init().await?;
    cli.command.execute(&mut engine).await?;

    Ok(())
}
