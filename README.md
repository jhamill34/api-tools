# API Caller Tools

> API requests on demand!

## Demo 

![demo](assets/demo.gif)

## Installing

### Homebrew 

Make sure [brew](https://brew.sh/) is installed.

```
brew tap jhamill34/tools 
brew install apicli
```

Homebrew should do most of the configuration for you. To start `apid` in the background
edit `$(brew --prefix apicli)/apid-config.toml` and change `connector.path` to the location 
you keep your local connectors. 

Then just run:
```
brew services start apicli
```

(Recommended) To use `apicli` copy the installed default config to `$HOME/.apicli/config.toml`

```
cp $(brew --prefix apicli)/apicli-config.toml $HOME/.apicli/config.toml
```

or set the `APICLI_CONFIG_PATH` environment variable

```
# make sure this is in your .bashrc or .zshrc as well..
export APICLI_CONFIG_PATH=$(brew --prefix apicli)/apicli-config.toml
```


### From Source

Building from source requires the [Cargo](https://doc.rust-lang.org/cargo/getting-started/installation.html) toolchain.

```
cargo install --path binary/apid --features "javascript, wrapper"
cargo install --path binary/apicli 
```

## Post Installation

### API Daemon 

> This is done for you if you used homebrew, just edit the `connector.path` value.

Before starting the daemon we need to set the `APID_CONFIG_PATH` environment variable to point to our config file. 

Example config file: 

```toml 
[connector]
path = "<CHANGE ME>"

[log]
api_path = "/usr/local/var/log/apid/api.log"
workflow_path = "/usr/local/var/log/apid/workflow.log"

[server]
port = 50051
host = "0.0.0.0"
```

### APICLI 

> This is done for you if you used homebrew, just follow the instructions above to make sure you're pointing to the correct file.

The client CLI also uses a config file, this file by default needs to be located at `$HOME/.apicli/config.toml`. This 
can be overridden by setting the environment variable `APICLI_CONFIG_PATH`. 

Default config file: 

```toml
[oauth]
base_uri = "https://apicli.localhost:8000"
cert_path = "/usr/local/apicli/certs/apicli.localhost.pem"
key_path = "/usr/local/apicli/certs/apicli.localhost-key.pem"

[template]
path = "/usr/local/apicli/templates"

[client]
port = 50051
host = "localhost"
```

The `oauth` config is for authenticating connectors and is for the callback of those configurations. When 
setting up an OAUTH application the `redirect_uri` needs to be `<base_uri>/oauth/callback`. Many oauth apps
require an SSL config which means local certs need to be configured. We ship our bundle with certs that are tied to
`apicli.localhost`.

If you prefer to make your own certs, I like to use [mkcert](https://github.com/FiloSottile/mkcert)

```bash
mkcert "your base_uri"
mkcert -install
```

> Don't forget to add your `base_uri` to your `/etc/hosts` file!

This will create a `cert` file and `key` file which you can put anywhere you like, just update the above configuration to point to them. 

## Usage

The contents of this tool contains thee parts:

- `apid`
- `apicli`
- Supplemental scripts

### API Daemon

The purpose of this daemon is to efficiently watch changes to local connectors and actions 
and serve a similar purpose as a Language Server. It should run in the background on port `:50051`.

> Current design doesn't pick up new actions to watch so you'll need to restart the daemon to pick them up

To start just run: 

```
apid
```

Each connector loaded / watched is expected to have:

| File | Description |
|:---|:---|
| `manifest.json` | This file represents the type of action/connector to run |
| `credentials.json` | A credential file used to store the credentials for connecting to the connector | 
| `config.json` | A configuration file for any overrides | 


#### credentials.json 

*Header Example: *

```json 
{
	"header": {
		"value": "...."
	}
}
```

*Oauth Example: *

```json 
{
	"oauth": {
		"clientId": "....",
		"clientSecret": "...."
	}
}
```

### Supplemental Scripts

Most of these scripts are either one-of bash scripts or are bash scripts built on top of `apicli` 
to extend functionality.

#### API-Lite

This script expects `fzf` and `jq` installed. 

```
apilite 
```

This script gets a list of all loaded operations, and pipes it into `fzf`. You can then select which 
operation to execute. It then opens up your default editor (i.e. `$EDITOR`) to provide your input. 
The default value is a stub if its never been provided before otherwise its the last input provided. 

The script then waits for the result and prints it out to the command line.

#### API-Gen 

```
apigen -t TEMPLATE_NAME -n NAME [-c USER_NAME]
```

This script gets input and output paths and pipes them into `fzf -m` to multi-select these paths and dumps them into a
file. The script then opens up your default editor (i.e. `$EDITOR`) to complete the mappings. Lastly, 
it then generates an action based on the provided inputs. 

#### Swagger Convert 

```
swagger_convert @./swagger.yaml > openapi.yaml
```

### APICLI 

This is the core of our tooling functionality. Most of the commands connect to the API Daemon 
and acts as our client into interacting with our local connectors and actions. 

#### Commands 

##### List 

```
apicli list 
```

Prints out a list of all available operations that we have access to. 

##### Get 

```
apicli get NAME
```

Prints out the manifest file of the requested service.

##### Oauth 

```
apicli oauth NAME
```

##### Input/Output Stub 

```
apicli input-stub NAME
apicli output-stub NAME
```

Based off of the manifest or OpenAPI spec, this command prints a JSON stub representing a possible input or output payload. 

##### Input/Output Paths

```
apicli input-paths NAME
apicli output-paths NAME
```

Based off of the manifest or OpenAPI spec, this command prints an enumerated list of simple JMESPaths that represent 
the input or output payload. 

##### Schema 

```
apicli schema [INPUT_FILE]
```

Given a JSON payload, will print out an OpenAPI complient schema. 

The `INPUT_FILE` could be directly provided or the input is read through `stdin`.

##### Merge 

```
apicli merge LEFT_FILE RIGHT_FILE
```

Given two files that represents OpenAPI schemas, this command will merge the two so that the new schema 
will match either input.

##### Run 

> Not recommneded to run directly, use `apilite`

```
apicli run NAME [INPUT_FILE] [--limit number]
```

The run command asynchronously runs the operation and returns an `execution_id`.

The `INPUT_FILE` could be directly provided or the input is read through `stdin`.

##### RunStatus

> Not recommneded to run directly, use `apilite`

```
apicli run-status EXECUTION_ID
```

Since the run command asynchronously runs, we can provide the execution id to get the status. 

##### RunResult 

> Not recommneded to run directly, use `apilite`

```
apicli run-result EXECUTION_ID
```

Since the run command asynchronously runs, we can provide the execution id to get the response body in JSON. 

##### Generate 

> Not recommneded to run directly, use `apigen`

```
apicli generate TEMPLATE_NAME NAME API [INPUT]
```

Input is a file that represents a mapping of the new action and a JMESPath to map to the inner action.


The input looks like the following:

```
input_name -> path_to_request <SCHEMA>
output_name <- path.to.response.body <SCHEMA>
```

