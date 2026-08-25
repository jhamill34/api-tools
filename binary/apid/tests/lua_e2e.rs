//! End-to-end smoke test: does a registered Lua `SimpleCode` operation
//! actually run correctly through a real `apid` daemon?
//!
//! `lua_runner`'s own unit tests already exercise `LuaActionRunner`
//! directly (including its `api.run` binding against a real `Engine`), but
//! nothing before this proved the daemon actually wires a Lua script up
//! end to end: background-loads a `manifest.json` from disk, dispatches a
//! `RunService` gRPC call through `execution_engine::Engine::run`'s `LUA`
//! arm, and returns the real result over `GetRunResult`. Complements the
//! startup-only smoke test in `.github/workflows/rust.yml`, which proves
//! the binary starts but never exercises a registered operation.
#![cfg(feature = "lua")]

use std::{
    process::{Child, Command, Stdio},
    time::Duration,
};

use engine_entities::engine::{
    engine_client::EngineClient, get_run_result_response, GetRunResultRequest, ListRequest,
    RunServiceRequest,
};
use tempfile::TempDir;
use tokio::{net::TcpStream, time::Instant};

const PORT: u16 = 50097;

/// Kills the spawned `apid` process on drop, so a failed assertion
/// (which unwinds through this guard) doesn't leave an orphaned daemon
/// running.
struct ApidProcess(Child);

impl Drop for ApidProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Writes a `lua_smoke` connector (a `SimpleCode` Lua manifest) and a
/// matching `config.toml` under `root`.
fn write_fixture(root: &std::path::Path) {
    let connector_dir = root.join("connectors").join("lua_smoke");
    std::fs::create_dir_all(&connector_dir).expect("create connector dir");

    let manifest = r#"{
  "v2": {
    "simpleCode": {
      "code": {
        "codeString": "local input = ...\nreturn { greeting = 'hello ' .. input.name, doubled = input.value * 2 }",
        "language": "LUA"
      }
    }
  }
}"#;
    std::fs::write(connector_dir.join("manifest.json"), manifest).expect("write manifest");

    let config = format!(
        "[connector]\npath = \"{}\"\n\n[log]\napi_path = \"{}\"\nworkflow_path = \"{}\"\n\n[server]\nport = {PORT}\nhost = \"127.0.0.1\"\n",
        root.join("connectors").display(),
        root.join("api.log").display(),
        root.join("workflow.log").display(),
    );
    std::fs::write(root.join("config.toml"), config).expect("write config");
}

async fn wait_for_port(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test]
async fn lua_simple_code_runs_end_to_end_through_a_real_apid_daemon() {
    let dir = TempDir::new().expect("create tempdir");
    write_fixture(dir.path());

    let apid_log = std::fs::File::create(dir.path().join("apid.log")).expect("create log file");
    let child = Command::new(env!("CARGO_BIN_EXE_apid"))
        .env("APID_CONFIG_PATH", dir.path().join("config.toml"))
        .env("HOME", dir.path())
        .stdout(Stdio::from(apid_log.try_clone().expect("clone log handle")))
        .stderr(Stdio::from(apid_log))
        .spawn()
        .expect("spawn apid");
    let _apid = ApidProcess(child);

    let log_contents = || std::fs::read_to_string(dir.path().join("apid.log")).unwrap_or_default();

    assert!(
        wait_for_port(PORT, Duration::from_secs(15)).await,
        "apid never bound its gRPC port:\n{}",
        log_contents()
    );

    let mut client = EngineClient::connect(format!("http://127.0.0.1:{PORT}"))
        .await
        .unwrap_or_else(|err| panic!("failed to connect to apid: {err}\n{}", log_contents()));

    // The background watcher/loader loads connectors asynchronously, so
    // the fixture connector may not be visible immediately after the
    // gRPC port opens - poll List() until it shows up.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let list = client
            .list(ListRequest {})
            .await
            .expect("list rpc failed")
            .into_inner();

        if list
            .items
            .iter()
            .any(|item| item.name == "(code) lua_smoke.execute")
        {
            break;
        }

        assert!(
            Instant::now() < deadline,
            "lua_smoke connector was never loaded by the background watcher:\n{}",
            log_contents()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let run = client
        .run_service(RunServiceRequest {
            id: "lua_smoke.execute".to_owned(),
            input: serde_json::json!({ "name": "world", "value": 21 }).to_string(),
            limit: None,
            execution_id: None,
        })
        .await
        .expect("run_service rpc failed")
        .into_inner();
    assert!(!run.execution_id.is_empty());

    let deadline = Instant::now() + Duration::from_secs(15);
    let output = loop {
        let result = client
            .get_run_result(GetRunResultRequest {
                execution_id: run.execution_id.clone(),
            })
            .await
            .expect("get_run_result rpc failed")
            .into_inner();

        if result.status() == get_run_result_response::Status::Completed {
            break result.output.expect("completed run has no output");
        }

        assert!(
            Instant::now() < deadline,
            "run never completed (status {:?}):\n{}",
            result.status(),
            log_contents()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    let parsed: serde_json::Value = serde_json::from_str(&output)
        .unwrap_or_else(|err| panic!("output wasn't JSON: {err}\n{output}"));

    assert_eq!(
        parsed,
        serde_json::json!([{ "greeting": "hello world", "doubled": 42 }]),
        "unexpected output from the real daemon:\n{}",
        log_contents()
    );
}
