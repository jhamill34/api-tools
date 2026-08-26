#![allow(
    clippy::print_stdout,
    reason = "this CLI's actual output mechanism for command results"
)]

//! Handlers for every CLI subcommand: the gRPC client calls to `apid`
//! ([`Cli`]), the local JSON-schema inference/merge helpers, and the
//! `generate` command's template rendering.

use serde::{Deserialize, Serialize};
use tera::{Context, Tera};

use std::{
    collections::HashMap,
    env, fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, Context as _};
use core_entities::service::VersionedServiceTree;
use credential_entities::credentials::Authentication;
use engine_entities::engine::{
    engine_client::EngineClient, GetRunResultRequest, GetSerivceRequest, ListRequest,
    ProvideInputRequest, RunServiceRequest, SaveServiceRequest,
};
use oauth_flow::Authenticator;
use protobuf::Message;
use protobuf_json_mapping::PrintOptions;
use tonic::{transport::Channel, Request};

use crate::{
    config::Configuration,
    constants,
    path::{get_input_paths, get_output_paths},
    stub::{get_input, get_output},
    template::{Direction, InputDescription},
};

/// Reads a single line from stdin.
fn read_line() -> io::Result<String> {
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(input)
}

/// Reads lines from stdin until a blank line, joining them with `\n`. Used
/// as the fallback input source for commands that also accept a file path.
fn read_lines_from_stdin() -> io::Result<String> {
    let mut lines: Vec<String> = Vec::new();

    let mut line = read_line()?;
    while !line.trim().is_empty() {
        lines.push(line);
        line = read_line()?;
    }

    Ok(lines.join("\n"))
}

/// Dispatches every CLI subcommand's logic: gRPC calls to `apid`, local
/// stub/path/schema generation, and template rendering.
pub struct Cli {
    /// The gRPC client connected to the configured `apid` daemon.
    client: EngineClient<Channel>,

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
        let client = EngineClient::connect(endpoint).await?;

        Ok(Cli { client, config })
    }

    /// Lists every operation of every loaded service.
    pub async fn handle_list(&mut self) -> anyhow::Result<()> {
        let request = Request::new(ListRequest {});

        let response = self.client.list(request).await?.into_inner();

        for item in response.items {
            println!("{}", item.name);
        }

        Ok(())
    }

    /// Prints a service's manifest as pretty-printed JSON (with default
    /// field values included).
    pub async fn handle_get_service(&mut self, name: String) -> anyhow::Result<()> {
        let request = Request::new(GetSerivceRequest { name });
        let response = self.client.get_service(request).await?.into_inner();

        let service = VersionedServiceTree::parse_from_bytes(&response.raw_service)?;
        let service = service.v1();
        let manifest = service.manifest.v2();

        let options = PrintOptions {
            always_output_default_values: true,
            ..Default::default()
        };
        let manifest = protobuf_json_mapping::print_to_string_with_options(manifest, &options)?;
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

        let request = Request::new(GetSerivceRequest { name: name.clone() });
        let response = self.client.get_service(request).await?.into_inner();

        let credentials = response
            .raw_credentials
            .ok_or_else(|| anyhow!("Expected the service to have credentials"))?;
        let credentials = Authentication::parse_from_bytes(&credentials)?;

        let credentials = Arc::new(Mutex::new(credentials));
        let service = VersionedServiceTree::parse_from_bytes(&response.raw_service)?;

        let auth = Authenticator::new(base_path, key_path, cert_path);
        auth.start(name.clone(), service, Arc::clone(&credentials))
            .await?;

        let raw_credentials = {
            let credentials = credentials
                .lock()
                .map_err(|e| anyhow!("Credentials Lock has been poisoned: {e}"))?;
            credentials.write_to_bytes()?
        };

        let save_request = Request::new(SaveServiceRequest {
            name,
            raw_service: None,
            raw_credentials: Some(raw_credentials),
        });

        self.client.save_service(save_request).await?;

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

        let request = Request::new(RunServiceRequest {
            id: name.clone(),
            input,
            limit,
            execution_id: None,
        });
        let response = self.client.run_service(request).await?.into_inner();

        println!("{}", response.execution_id);

        Ok(())
    }

    /// Prints a run's output if it's completed or waiting on input, or
    /// `{}` if it's not found, still running, or errored.
    pub async fn handle_run_result(&mut self, execution_id: String) -> anyhow::Result<()> {
        let request = Request::new(GetRunResultRequest { execution_id });
        let response = self.client.get_run_result(request).await?.into_inner();

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
        let request = Request::new(GetRunResultRequest { execution_id });
        let response = self.client.get_run_result(request).await?.into_inner();

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

    /// Answers a run's pending `InputPrompter` prompt with `input` (or
    /// read from stdin if omitted).
    pub async fn handle_provide_input(
        &mut self,
        execution_id: String,
        input: Option<String>,
    ) -> anyhow::Result<()> {
        let input = if let Some(input) = input {
            fs::read_to_string(Path::new(&input))?
        } else {
            read_lines_from_stdin()?
        };

        let request = Request::new(ProvideInputRequest {
            execution_id,
            input,
        });
        let _response = self.client.provide_input(request).await?.into_inner();

        Ok(())
    }

    /// Fetches `id`'s (`{service}.{operation}`) manifest and prints a
    /// sample JSON input payload via [`stub::get_input`](crate::stub::get_input).
    pub async fn handle_input_stub(&mut self, id: String, required: bool) -> anyhow::Result<()> {
        let parts: Vec<_> = id.split('.').collect();

        let name = parts
            .first()
            .ok_or_else(|| anyhow!("Expected a service name"))?;
        let name = (*name).to_owned();

        let operation = parts
            .get(1)
            .ok_or_else(|| anyhow!("Expected an operation name"))?;

        let request = Request::new(GetSerivceRequest { name });
        let response = self.client.get_service(request).await?.into_inner();

        let service = VersionedServiceTree::parse_from_bytes(&response.raw_service)?;

        let stub = get_input(&service, operation, required)?;
        let stub = serde_json::to_string_pretty(&stub)?;

        println!("{stub}");

        Ok(())
    }

    /// Fetches `id`'s (`{service}.{operation}`) manifest and prints a
    /// sample JSON output payload via [`stub::get_output`](crate::stub::get_output).
    pub async fn handle_output_stub(&mut self, id: String) -> anyhow::Result<()> {
        let parts: Vec<_> = id.split('.').collect();

        let name = parts
            .first()
            .ok_or_else(|| anyhow!("Expected a service name"))?;
        let name = (*name).to_owned();

        let operation = parts
            .get(1)
            .ok_or_else(|| anyhow!("Expected an operation name"))?;

        let request = Request::new(GetSerivceRequest { name });
        let response = self.client.get_service(request).await?.into_inner();

        let service = VersionedServiceTree::parse_from_bytes(&response.raw_service)?;

        let stub = get_output(&service, operation)?;
        let stub = serde_json::to_string_pretty(&stub)?;

        println!("{stub}");

        Ok(())
    }

    /// Fetches `id`'s (`{service}.{operation}`) manifest and prints its
    /// input fields as a flat listing via
    /// [`path::get_input_paths`](crate::path::get_input_paths).
    pub async fn handle_input_paths(&mut self, id: String, required: bool) -> anyhow::Result<()> {
        let parts: Vec<_> = id.split('.').collect();

        let name = parts
            .first()
            .ok_or_else(|| anyhow!("Expected a service name"))?;
        let name = (*name).to_owned();

        let operation = parts
            .get(1)
            .ok_or_else(|| anyhow!("Expected an operation name"))?;

        let request = Request::new(GetSerivceRequest { name });
        let response = self.client.get_service(request).await?.into_inner();

        let service = VersionedServiceTree::parse_from_bytes(&response.raw_service)?;

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
        let parts: Vec<_> = id.split('.').collect();

        let name = parts
            .first()
            .ok_or_else(|| anyhow!("Expected a service name"))?;
        let name = (*name).to_owned();

        let operation = parts
            .get(1)
            .ok_or_else(|| anyhow!("Expected an operation name"))?;

        let request = Request::new(GetSerivceRequest { name });
        let response = self.client.get_service(request).await?.into_inner();

        let service = VersionedServiceTree::parse_from_bytes(&response.raw_service)?;

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

/// Infers a YAML schema from a JSON example payload (`input`, or read from
/// stdin if omitted) via [`schemaify`] and prints it.
pub fn handle_schema_convert(input: Option<String>) -> anyhow::Result<()> {
    let input = if let Some(input) = input {
        fs::read_to_string(Path::new(&input))?
    } else {
        read_lines_from_stdin()?
    };

    let input = serde_json::from_str(&input)?;
    let schema = schemaify(&input);

    let schema = serde_yaml::to_string(&schema)?;

    println!("{schema}");

    Ok(())
}

/// Reads two YAML schema files and prints their [`merge`]d union.
pub fn handle_schema_merge(left: &str, right: &str) -> anyhow::Result<()> {
    let left = fs::read_to_string(Path::new(&left))?;
    let left: Schema = serde_yaml::from_str(&left)?;

    let right = fs::read_to_string(Path::new(&right))?;
    let right: Schema = serde_yaml::from_str(&right)?;

    let merged = merge(left, right);
    let merged = serde_yaml::to_string(&merged)?;

    println!("{merged}");

    Ok(())
}

/// An inferred JSON schema: either a single concrete type, or a `oneOf`
/// composition when the same position held incompatible types.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(untagged)]
enum Schema {
    /// A single concrete type.
    Single(SchemaObject),

    /// A `oneOf` composition of multiple possible types.
    Composite(SchemaComposite),
}

/// A `oneOf` composition of possible schemas.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct SchemaComposite {
    /// The possible schemas, deduplicated.
    one_of: Vec<Schema>,
}

/// A single concrete inferred type.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
enum SchemaObject {
    /// Inferred from a JSON `null`.
    Null,

    /// Inferred from a JSON boolean.
    Boolean,

    ///s
    Number,

    /// Inferred from a JSON string.
    String,

    /// Inferred from a JSON object.
    Object {
        /// Each property's inferred schema, keyed by name.
        properties: HashMap<String, Schema>,
    },

    /// Inferred from a JSON array, with every element's schema merged
    /// into one.
    Array {
        /// The merged element schema.
        items: Box<Schema>,
    },
}

/// Infers a [`Schema`] from a JSON value: a concrete type for a scalar, a
/// per-key schema for an object, or the [`merge`]d schema of every element
/// for an array (an empty array infers an empty object, since there's
/// nothing to merge).
fn schemaify(value: &serde_json::Value) -> Schema {
    match value {
        &serde_json::Value::Null => Schema::Single(SchemaObject::Null),
        &serde_json::Value::Bool(_) => Schema::Single(SchemaObject::Boolean),
        &serde_json::Value::Number(_) => Schema::Single(SchemaObject::Number),
        &serde_json::Value::String(_) => Schema::Single(SchemaObject::String),
        &serde_json::Value::Object(ref obj) => {
            let mut properties = HashMap::new();

            for (key, value) in obj {
                properties.insert(key.clone(), schemaify(value));
            }

            Schema::Single(SchemaObject::Object { properties })
        }
        &serde_json::Value::Array(ref arr) => {
            let result = arr.iter().map(schemaify).reduce(merge);

            if let Some(result) = result {
                Schema::Single(SchemaObject::Array {
                    items: Box::new(result),
                })
            } else {
                Schema::Single(SchemaObject::Object {
                    properties: HashMap::new(),
                })
            }
        }
    }
}

/// Merges two schemas into one: identical schemas merge to themselves;
/// two objects merge property-by-property (a property present on only one
/// side is kept as-is); two arrays merge their item schemas; anything else
/// incompatible becomes (or extends) a `oneOf` composition of the
/// distinct schemas seen.
fn merge(left: Schema, right: Schema) -> Schema {
    if left == right {
        left
    } else {
        match &left {
            &Schema::Single(SchemaObject::Object { ref properties }) => match &right {
                &Schema::Single(SchemaObject::Object {
                    properties: ref right_properties,
                }) => {
                    let mut existing = HashMap::new();

                    for (key, value) in properties {
                        if let Some(right_value) = right_properties.get(key) {
                            existing.insert(key.clone(), merge(value.clone(), right_value.clone()));
                        } else {
                            existing.insert(key.clone(), value.clone());
                        }
                    }

                    for (key, value) in right_properties {
                        if !existing.contains_key(key) {
                            existing.insert(key.clone(), value.clone());
                        }
                    }

                    Schema::Single(SchemaObject::Object {
                        properties: existing,
                    })
                }
                &Schema::Composite(SchemaComposite { ref one_of }) => {
                    let mut one_of = one_of.clone();

                    if !one_of.contains(&left) {
                        one_of.push(left);
                    }

                    Schema::Composite(SchemaComposite { one_of })
                }
                &Schema::Single(_) => Schema::Composite(SchemaComposite {
                    one_of: vec![left, right],
                }),
            },
            &Schema::Single(SchemaObject::Array { ref items }) => match &right {
                &Schema::Single(SchemaObject::Array {
                    items: ref right_items,
                }) => Schema::Single(SchemaObject::Array {
                    items: Box::new(merge((**items).clone(), (**right_items).clone())),
                }),
                &Schema::Composite(SchemaComposite { ref one_of }) => {
                    let mut one_of = one_of.clone();

                    if !one_of.contains(&left) {
                        one_of.push(left);
                    }

                    Schema::Composite(SchemaComposite { one_of })
                }
                &Schema::Single(_) => Schema::Composite(SchemaComposite {
                    one_of: vec![left, right],
                }),
            },
            &Schema::Composite(SchemaComposite { ref one_of }) => match &right {
                &Schema::Single(_) => {
                    let mut one_of = one_of.clone();
                    if !one_of.contains(&right) {
                        one_of.push(right);
                    }

                    Schema::Composite(SchemaComposite { one_of })
                }
                &Schema::Composite(SchemaComposite {
                    one_of: ref right_one_of,
                }) => {
                    let mut one_of = one_of.clone();
                    for right_value in right_one_of {
                        if !one_of.contains(right_value) {
                            one_of.push(right_value.clone());
                        }
                    }

                    Schema::Composite(SchemaComposite { one_of })
                }
            },
            &Schema::Single(_) => match &right {
                &Schema::Single(_) => Schema::Composite(SchemaComposite {
                    one_of: vec![left, right],
                }),
                &Schema::Composite(SchemaComposite { ref one_of }) => {
                    let mut one_of = one_of.clone();
                    if !one_of.contains(&left) {
                        one_of.push(left.clone());
                    }

                    Schema::Composite(SchemaComposite { one_of })
                }
            },
        }
    }
}
