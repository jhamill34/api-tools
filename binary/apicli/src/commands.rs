//! Command-line argument parsing (via `clap`), and dispatch of each
//! subcommand to its handler on [`engine::Cli`].

use crate::engine;
use crate::schema::{handle_schema_convert, handle_schema_merge};
use clap::{Parser, Subcommand};

/// Top-level CLI arguments.
#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Commands,
}

/// Every subcommand this CLI supports.
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Lists every operation of every loaded service.
    List,

    /// Prints a service's manifest and credentials, fetched from the
    /// daemon.
    Get {
        /// The service name.
        name: String,
    },

    /// Runs the interactive OAuth login flow for a service and saves the
    /// resulting credentials.
    Oauth {
        /// The service name.
        name: String,
    },

    /// Invokes a `{service}.{operation}` and prints the resulting
    /// execution ID.
    Run {
        /// The `{service}.{operation}` identifier to invoke.
        name: String,

        /// The JSON input, or read from stdin if omitted.
        input: Option<String>,

        /// Caps the number of paginated results returned.
        #[arg(short, long)]
        limit: Option<i32>,
    },

    /// Prints a previously started run's current status.
    RunStatus {
        /// The execution ID returned by [`Run`](Commands::Run).
        execution_id: String,
    },

    /// Prints a previously started run's result, if it has completed.
    RunResult {
        /// The execution ID returned by [`Run`](Commands::Run).
        execution_id: String,
    },

    /// Prints a sample JSON input payload for an operation.
    InputStub {
        /// The `{service}.{operation}` identifier.
        name: String,

        /// Include only required fields.
        #[arg(short, long, default_value_t = false)]
        required: bool,
    },

    /// Prints a sample JSON output payload for an operation.
    OutputStub {
        /// The `{service}.{operation}` identifier.
        name: String,
    },

    /// Prints a flat listing of an operation's input fields (path, type,
    /// context, description).
    InputPaths {
        /// The `{service}.{operation}` identifier.
        name: String,

        /// Include only required fields.
        #[arg(short, long, default_value_t = false)]
        required: bool,
    },

    /// Prints a flat listing of an operation's output fields (path, type,
    /// context, description).
    OutputPaths {
        /// The `{service}.{operation}` identifier.
        name: String,
    },

    /// Converts a JSON example payload into an inferred YAML schema.
    Schema {
        /// Path to the JSON payload, or read from stdin if omitted.
        input: Option<String>,
    },

    /// Merges two YAML schema files into one.
    Merge {
        /// Path to the first schema file.
        left: String,

        /// Path to the second schema file.
        right: String,
    },

    /// Scaffolds a new service definition from a template.
    Generate {
        /// The scaffolding template to use.
        template_name: String,

        /// The new service's name.
        name: String,

        /// The API name to scaffold against.
        api: String,

        /// Template-specific JSON input, or read from stdin if omitted.
        input: Option<String>,
    },
}

impl Commands {
    /// Dispatches this command to its handler on `engine`.
    pub async fn execute(self, engine: &mut engine::Cli) -> anyhow::Result<()> {
        match self {
            Self::List => engine.handle_list().await?,
            Self::Get { name } => engine.handle_get_service(name).await?,
            Self::Oauth { name } => engine.handle_auth(name).await?,
            Self::Run { name, input, limit } => engine.handle_run(name, input, limit).await?,
            Self::RunResult { execution_id } => engine.handle_run_result(execution_id).await?,
            Self::RunStatus { execution_id } => engine.handle_run_status(execution_id).await?,
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
