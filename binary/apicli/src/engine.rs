#![allow(
    clippy::print_stdout,
    reason = "this CLI's actual output mechanism for command results"
)]

//! Handlers for every CLI subcommand that talks to `apid`: the gRPC calls
//! ([`Cli`]), plus the `generate` command's local template rendering. Local
//! JSON-schema inference/merging (which never talks to `apid`) lives in
//! [`crate::schema`] instead.

use serde::{Deserialize, Serialize};
use tera::{Context, Tera};

use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, Context as _};
use core_entities::service::VersionedServiceTree;
use credential_entities::credentials::Authentication;
use grpc_client::EngineGrpcClient;
use oauth_flow::Authenticator;

use crate::{
    config::Configuration,
    constants,
    io_util::read_lines_from_stdin,
    path::{get_input_paths, get_output_paths},
    stub::{get_input, get_output},
    template::{Direction, InputDescription},
};

/// Dispatches every CLI subcommand's logic that talks to `apid`: gRPC calls,
/// local stub/path generation over the fetched manifest, and template
/// rendering.
pub struct Cli {
    /// The gRPC client connected to the configured `apid` daemon.
    client: EngineGrpcClient,

    /// The loaded CLI configuration.
    config: Configuration,
}

impl Cli {
    /// Loads the CLI's configuration (from
    /// [`constants::APICLI_CONFIG_PATH`], defaulting to
    /// `~/.apicli/config.toml`) and connects to the configured `apid`
    /// daemon.
    pub async fn init() -> anyhow::Result<Self> {
        let config = env::var(constants::APICLI_CONFIG_PATH).unwrap_or_else(|_| {
            let home = env::var("HOME").unwrap_or_else(|_| ".".to_owned());

            format!("{home}/.apicli/config.toml")
        });
        let config = fs::read_to_string(&config)
            .with_context(|| format!("Failed to read config file at {config}"))?;

        let config: Configuration = toml::from_str(&config)?;

        let endpoint = format!("http://{}:{}", config.client.host, config.client.port);
        let client = EngineGrpcClient::connect(endpoint).await?;

        Ok(Cli { client, config })
    }

    /// Lists every operation of every loaded service.
    pub async fn handle_list(&mut self) -> anyhow::Result<()> {
        for name in self.client.list().await? {
            println!("{name}");
        }

        Ok(())
    }

    /// Prints a service's manifest as pretty-printed JSON (with default
    /// field values included).
    pub async fn handle_get_service(&mut self, name: String) -> anyhow::Result<()> {
        let response = self.client.get_service(name).await?;

        let service: VersionedServiceTree = serde_json::from_slice(&response.raw_service)?;
        let v1 = service.v1();
        let manifest = v1.manifest_latest();

        let manifest = serde_json::to_string_pretty(&manifest)?;
        println!("{manifest}");
        Ok(())
    }

    /// Runs the interactive OAuth login flow for `name` (via the embedded
    /// [`oauth_flow::Authenticator`] web server) and saves the resulting
    /// credentials back to the daemon.
    pub async fn handle_auth(&mut self, name: String) -> anyhow::Result<()> {
        let base_path = self.config.oauth.base_uri.clone();
        let key_path = self.config.oauth.key_path.clone();
        let cert_path = self.config.oauth.cert_path.clone();

        let response = self.client.get_service(name.clone()).await?;

        let credentials = response
            .raw_credentials
            .ok_or_else(|| anyhow!("Expected the service to have credentials"))?;
        let credentials: Authentication = serde_json::from_slice(&credentials)?;

        let credentials = Arc::new(Mutex::new(credentials));
        let service: VersionedServiceTree = serde_json::from_slice(&response.raw_service)?;

        let auth = Authenticator::new(base_path, key_path, cert_path);
        auth.start(name.clone(), service, Arc::clone(&credentials))
            .await?;

        let raw_credentials = {
            let credentials = credentials
                .lock()
                .map_err(|e| anyhow!("Credentials Lock has been poisoned: {e}"))?;
            serde_json::to_vec(&*credentials)?
        };

        self.client
            .save_service(name, None, Some(raw_credentials))
            .await?;

        println!("Done!");

        Ok(())
    }

    /// Starts running `{service}.{operation}` (from `input`, or read from
    /// stdin if omitted, capped at `limit` results) and prints the
    /// resulting execution ID.
    pub async fn handle_run(
        &mut self,
        name: String,
        input: Option<String>,
        limit: Option<i32>,
    ) -> anyhow::Result<()> {
        let input = if let Some(input) = input {
            fs::read_to_string(Path::new(&input))?
        } else {
            read_lines_from_stdin()?
        };

        let execution_id = self.client.run_service(name, input, limit).await?;

        println!("{execution_id}");

        Ok(())
    }

    /// Prints a run's output if it's completed or waiting on input, or
    /// `{}` if it's not found, still running, or errored.
    pub async fn handle_run_result(&mut self, execution_id: String) -> anyhow::Result<()> {
        let response = self.client.get_run_result(execution_id).await?;

        match response.status() {
            engine_entities::engine::get_run_result_response::Status::Completed
            | engine_entities::engine::get_run_result_response::Status::Waiting => {
                println!("{}", response.output());
            }
            engine_entities::engine::get_run_result_response::Status::NotFound
            | engine_entities::engine::get_run_result_response::Status::Running
            | engine_entities::engine::get_run_result_response::Status::Error => {
                println!("{{}}");
            }
        }

        Ok(())
    }

    /// Prints a run's current status (`Not Found`/`Running`/`Error`/
    /// `Completed`/`Waiting`).
    pub async fn handle_run_status(&mut self, execution_id: String) -> anyhow::Result<()> {
        let response = self.client.get_run_result(execution_id).await?;

        match response.status() {
            engine_entities::engine::get_run_result_response::Status::NotFound => {
                println!("Not Found");
            }
            engine_entities::engine::get_run_result_response::Status::Running => {
                println!("Running");
            }
            engine_entities::engine::get_run_result_response::Status::Error => {
                println!("Error");
            }
            engine_entities::engine::get_run_result_response::Status::Completed => {
                println!("Completed");
            }
            engine_entities::engine::get_run_result_response::Status::Waiting => {
                println!("Waiting");
            }
        }

        Ok(())
    }

    /// Fetches `id`'s (`{service}.{operation}`) manifest and prints a
    /// sample JSON input payload via [`stub::get_input`](crate::stub::get_input).
    pub async fn handle_input_stub(&mut self, id: String, required: bool) -> anyhow::Result<()> {
        let (name, operation) = split_service_operation(&id)?;

        let response = self.client.get_service(name).await?;
        let service: VersionedServiceTree = serde_json::from_slice(&response.raw_service)?;

        let stub = get_input(&service, operation, required)?;
        let stub = serde_json::to_string_pretty(&stub)?;

        println!("{stub}");

        Ok(())
    }

    /// Fetches `id`'s (`{service}.{operation}`) manifest and prints a
    /// sample JSON output payload via [`stub::get_output`](crate::stub::get_output).
    pub async fn handle_output_stub(&mut self, id: String) -> anyhow::Result<()> {
        let (name, operation) = split_service_operation(&id)?;

        let response = self.client.get_service(name).await?;
        let service: VersionedServiceTree = serde_json::from_slice(&response.raw_service)?;

        let stub = get_output(&service, operation)?;
        let stub = serde_json::to_string_pretty(&stub)?;

        println!("{stub}");

        Ok(())
    }

    /// Fetches `id`'s (`{service}.{operation}`) manifest and prints its
    /// input fields as a flat listing via
    /// [`path::get_input_paths`](crate::path::get_input_paths).
    pub async fn handle_input_paths(&mut self, id: String, required: bool) -> anyhow::Result<()> {
        let (name, operation) = split_service_operation(&id)?;

        let response = self.client.get_service(name).await?;
        let service: VersionedServiceTree = serde_json::from_slice(&response.raw_service)?;

        let paths = get_input_paths(&service, operation, required)?;

        for path in paths {
            println!(
                "{} <{}> {} , \"{}\"",
                path.path,
                path.type_,
                path.context.unwrap_or_default(),
                path.description
            );
        }

        Ok(())
    }

    /// Fetches `id`'s (`{service}.{operation}`) manifest and prints its
    /// output fields as a flat listing via
    /// [`path::get_output_paths`](crate::path::get_output_paths).
    pub async fn handle_output_paths(&mut self, id: String) -> anyhow::Result<()> {
        let (name, operation) = split_service_operation(&id)?;

        let response = self.client.get_service(name).await?;
        let service: VersionedServiceTree = serde_json::from_slice(&response.raw_service)?;

        let paths = get_output_paths(&service, operation)?;

        for path in paths {
            println!(
                "{} <{}> {} , \"{}\"",
                path.path,
                path.type_,
                path.context.unwrap_or_default(),
                path.description
            );
        }

        Ok(())
    }

    /// Parses `input_file` (or stdin) as newline-separated
    /// [`InputDescription`] lines, splits them into inputs and outputs,
    /// and renders every file in `template_name`'s template directory
    /// (under the configured template path) into a new `name` directory,
    /// with the parsed mappings available to the template as `inputs`/
    /// `outputs`.
    pub fn handle_generate(
        &self,
        template_name: &str,
        name: &str,
        api: &str,
        input_file: Option<String>,
    ) -> anyhow::Result<()> {
        let raw_input = if let Some(input_file) = input_file {
            fs::read_to_string(Path::new(&input_file))?
        } else {
            read_lines_from_stdin()?
        };

        let raw_input: anyhow::Result<Vec<InputDescription>> = raw_input
            .split('\n')
            .filter(|line| !line.is_empty())
            .map(str::parse)
            .collect();

        let (input, output): (Vec<_>, Vec<_>) = raw_input?
            .into_iter()
            .partition(|item| item.direction == Direction::Input);

        let model = TemplateModel {
            name: name.to_owned(),
            api: api.to_owned(),
            inputs: input,
            outputs: output,
        };
        let model = Context::from_serialize(model)?;

        let templates_dir = self.config.template.path.clone();
        let templates_dir = format!("{templates_dir}/{template_name}/**/*");

        fs::create_dir_all(name)?;

        let generate_root = PathBuf::from(name);

        let tera = Tera::new(&templates_dir)?;
        for template in tera.get_template_names() {
            let gen_path = generate_root.join(template);

            let dir = gen_path
                .parent()
                .ok_or_else(|| anyhow!("Expected a parent directory"))?;
            fs::create_dir_all(dir)?;

            let new_file = fs::File::create(&gen_path)?;

            tera.render_to(template, &model, new_file)?;
        }

        Ok(())
    }
}

/// Splits a `"{service}.{operation}"` identifier into its two parts,
/// erroring if either is missing. The shared parsing step of every handler
/// that operates on a single operation rather than a whole service.
fn split_service_operation(id: &str) -> anyhow::Result<(String, &str)> {
    let parts: Vec<_> = id.split('.').collect();

    let name = parts
        .first()
        .ok_or_else(|| anyhow!("Expected a service name"))?;
    let name = (*name).to_owned();

    let operation = parts
        .get(1)
        .ok_or_else(|| anyhow!("Expected an operation name"))?;

    Ok((name, operation))
}

/// The data a scaffolding template is rendered with.
#[derive(Debug, Serialize, Deserialize)]
struct TemplateModel {
    /// The new service's name.
    name: String,

    /// The API name being scaffolded against.
    api: String,

    /// The parsed input-mapping lines.
    inputs: Vec<InputDescription>,

    /// The parsed output-mapping lines.
    outputs: Vec<InputDescription>,
}
