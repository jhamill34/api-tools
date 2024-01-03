#![allow(clippy::print_stdout)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::needless_borrowed_reference)]

//!

extern crate alloc;
use alloc::sync::Arc;
use serde::{Deserialize, Serialize};
use tera::{Context, Tera};

use std::{
    collections::HashMap,
    env, fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
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

///
fn read_line() -> io::Result<String> {
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(input)
}

///
fn read_lines_from_stdin() -> io::Result<String> {
    let mut lines: Vec<String> = Vec::new();

    let mut line = read_line()?;
    while !line.trim().is_empty() {
        lines.push(line);
        line = read_line()?;
    }

    Ok(lines.join("\n"))
}

///
pub struct Cli {
    ///
    client: EngineClient<Channel>,

    ///
    config: Configuration,
}

impl Cli {
    ///
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

    ///
    pub async fn handle_list(&mut self) -> anyhow::Result<()> {
        let request = Request::new(ListRequest {});

        let response = self.client.list(request).await?.into_inner();

        for item in response.items {
            println!("{}", item.name);
        }

        Ok(())
    }

    ///
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

    ///
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

    ///
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

    ///
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

    ///
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

    ///
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

    ///
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

    ///
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

    ///
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

    ///
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

    ///
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

///
#[derive(Debug, Serialize, Deserialize)]
struct TemplateModel {
    ///
    name: String,

    ///
    api: String,

    ///
    inputs: Vec<InputDescription>,

    ///
    outputs: Vec<InputDescription>,
}

///
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

///
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

///
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(untagged)]
enum Schema {
    ///
    Single(SchemaObject),

    ///
    Composite(SchemaComposite),
}

///
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct SchemaComposite {
    ///
    one_of: Vec<Schema>,
}

///
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
enum SchemaObject {
    ///
    Null,

    ///
    Boolean,

    ///s
    Number,

    ///
    String,

    ///
    Object {
        ///
        properties: HashMap<String, Schema>,
    },

    ///
    Array {
        ///
        items: Box<Schema>,
    },
}

///
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

///
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
