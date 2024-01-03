//!

use crate::engine::{self, handle_schema_convert, handle_schema_merge};
use clap::{Parser, Subcommand};

///
#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    ///
    #[command(subcommand)]
    pub command: Commands,
}

///
#[derive(Debug, Subcommand)]
pub enum Commands {
    ///
    List,

    ///
    Get {
        ///
        name: String,
    },

    ///
    Oauth {
        ///
        name: String,
    },

    ///
    Run {
        ///
        name: String,

        ///
        input: Option<String>,

        ///
        #[arg(short, long)]
        limit: Option<i32>,
    },

    ///
    RunStatus {
        ///
        execution_id: String,
    },

    ///
    RunResult {
        ///
        execution_id: String,
    },

    ///
    ProvideInput {
        ///
        execution_id: String,

        ///
        input: Option<String>,
    },

    ///
    InputStub {
        ///
        name: String,

        ///
        #[arg(short, long, default_value_t = false)]
        required: bool,
    },

    ///
    OutputStub {
        ///
        name: String,
    },

    ///
    InputPaths {
        ///
        name: String,

        ///
        #[arg(short, long, default_value_t = false)]
        required: bool,
    },

    ///
    OutputPaths {
        ///
        name: String,
    },

    ///
    Schema {
        ///
        input: Option<String>,
    },

    ///
    Merge {
        ///
        left: String,

        ///
        right: String,
    },

    ///
    Generate {
        ///
        template_name: String,

        ///
        name: String,

        ///
        api: String,

        ///
        input: Option<String>,
    },
}

impl Commands {
    ///
    pub async fn execute(self, engine: &mut engine::Cli) -> anyhow::Result<()> {
        match self {
            Self::List => engine.handle_list().await?,
            Self::Get { name } => engine.handle_get_service(name).await?,
            Self::Oauth { name } => engine.handle_auth(name).await?,
            Self::Run { name, input, limit } => engine.handle_run(name, input, limit).await?,
            Self::RunResult { execution_id } => engine.handle_run_result(execution_id).await?,
            Self::RunStatus { execution_id } => engine.handle_run_status(execution_id).await?,
            Self::ProvideInput {
                execution_id,
                input,
            } => engine.handle_provide_input(execution_id, input).await?,
            Self::InputStub { name, required } => engine.handle_input_stub(name, required).await?,
            Self::OutputStub { name } => engine.handle_output_stub(name).await?,
            Self::InputPaths { name, required } => {
                engine.handle_input_paths(name, required).await?;
            }
            Self::OutputPaths { name } => engine.handle_output_paths(name).await?,
            Self::Schema { input } => handle_schema_convert(input)?,
            Self::Merge { left, right } => handle_schema_merge(&left, &right)?,
            Self::Generate {
                template_name,
                name,
                api,
                input,
            } => engine.handle_generate(&template_name, &name, &api, input)?,
        }

        Ok(())
    }
}
