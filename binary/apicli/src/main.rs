#![warn(clippy::restriction, clippy::pedantic)]
#![allow(
    clippy::blanket_clippy_restriction_lints,
    clippy::mod_module_files,
    clippy::self_named_module_files,
    clippy::implicit_return,
    clippy::shadow_reuse,
    clippy::match_ref_pats,
    clippy::shadow_unrelated,
    clippy::shadow_same,
    clippy::question_mark_used,
    // clippy::too_many_lines
    clippy::absolute_paths,
    clippy::single_call_fn,
    clippy::ref_patterns,

    clippy::min_ident_chars,
)]

//!

mod commands;
mod config;
mod constants;
mod engine;
mod path;
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
