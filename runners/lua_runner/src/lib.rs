#![warn(clippy::restriction, clippy::pedantic)]
#![allow(
    clippy::blanket_clippy_restriction_lints,
    clippy::mod_module_files,
    clippy::self_named_module_files,
    clippy::implicit_return,
    clippy::shadow_reuse,
    clippy::shadow_unrelated,
    clippy::match_ref_pats,
    clippy::separated_literal_suffix,
    clippy::question_mark_used,
    clippy::single_call_fn,
    clippy::absolute_paths
)]

//! A [`CodeRunner`] adapter that executes a Lua operation body in a fresh,
//! sandboxed, time-limited embedded [`mlua::Lua`] interpreter per call.
//!
//! **Sandboxing.** Only the `table`/`string`/`math` standard libraries are
//! loaded — no `io`, `os`, `package`, or `debug`. The Lua base library
//! (`error`, `pairs`, `pcall`, etc.) is always loaded regardless of that
//! selection and includes `dofile`/`loadfile`, which read arbitrary files
//! from disk despite `io` being disabled; both are explicitly removed
//! from the sandbox's globals after construction to close that gap.
//!
//! **Time budget.** Each run gets a fixed [`EXECUTION_TIMEOUT`], enforced
//! via an `mlua` instruction hook that checks elapsed wall-clock time
//! roughly every [`HOOK_INSTRUCTION_INTERVAL`] VM instructions; a script
//! that runs past the budget is aborted with an error.
//!
//! **Bindings.** Exposes one: `api.run(id, params, options)`, mirroring
//! `javascript_runner`'s surface — lets a script synchronously invoke
//! another registered operation. Unlike `python_runner`, there's no
//! `task`/`workflow`/`action` surface (no deferred/human-in-the-loop
//! scheduling, no structured outcome reporting distinct from the
//! script's return value) — deliberately out of scope for now, see #59.
//!
//! Constructing a fresh `Lua` interpreter per call is cheap enough
//! (measured ~60-150µs, no one-time boot cost) that — unlike
//! `python_runner`'s process-wide interpreter or `javascript_runner`'s
//! thread-local `MiniV8` — no reuse strategy is needed here.

extern crate alloc;
use alloc::sync::Arc;

use std::{
    sync::RwLock,
    time::{Duration, Instant},
};

mod constants;
pub mod error;

use common_data_structures::log_writer::LogWriter;
use execution_engine::services::{CodeRunner, EngineInputContext};
use mlua::{HookTriggers, Lua, LuaOptions, LuaSerdeExt, StdLib};

/// How long a Lua script is allowed to run before being aborted.
const EXECUTION_TIMEOUT: Duration = Duration::from_secs(5);

/// How often (in VM instructions) the execution-time budget is checked.
const HOOK_INSTRUCTION_INTERVAL: u32 = 1000;

/// A [`CodeRunner`] that executes a Lua operation body in a sandboxed,
/// time-limited [`Lua`] interpreter, with an `api.run` binding for
/// invoking other registered operations (see the crate-level docs for
/// what's deliberately not supported yet).
pub struct LuaActionRunner {
    /// The engine used to resolve `api.run` calls made from Lua.
    engine: Arc<RwLock<execution_engine::Engine>>,

    /// Where the `api.run` binding logs each nested call it makes.
    logger: LogWriter,
}

impl LuaActionRunner {
    /// Creates a [`LuaActionRunner`] that dispatches `api.run` calls
    /// through `engine` and logs them to `logger`.
    #[must_use]
    #[inline]
    pub fn new(engine: Arc<RwLock<execution_engine::Engine>>, logger: LogWriter) -> Self {
        Self { engine, logger }
    }

    /// Builds a sandboxed [`Lua`] instance with [`EXECUTION_TIMEOUT`]
    /// enforced (see the crate-level docs for exactly what's sandboxed).
    ///
    /// # Errors
    fn sandboxed_lua() -> error::Result<Lua> {
        Self::sandboxed_lua_with_timeout(EXECUTION_TIMEOUT)
    }

    /// Same as [`Self::sandboxed_lua`], but with `timeout` instead of the
    /// real [`EXECUTION_TIMEOUT`] — split out so tests can prove the
    /// abort mechanism itself works without waiting out the real budget.
    ///
    /// # Errors
    fn sandboxed_lua_with_timeout(timeout: Duration) -> error::Result<Lua> {
        let lua = Lua::new_with(
            StdLib::TABLE | StdLib::STRING | StdLib::MATH,
            LuaOptions::default(),
        )?;

        lua.globals().set("dofile", mlua::Value::Nil)?;
        lua.globals().set("loadfile", mlua::Value::Nil)?;

        let start = Instant::now();
        lua.set_hook(
            HookTriggers::default().every_nth_instruction(HOOK_INSTRUCTION_INTERVAL),
            move |_lua, _debug| {
                if start.elapsed() > timeout {
                    return Err(mlua::Error::RuntimeError(
                        "script exceeded its execution time budget".into(),
                    ));
                }

                Ok(())
            },
        );

        Ok(lua)
    }

    /// Installs the `api.run(id, params, options)` binding into `lua`,
    /// dispatching through `self.engine` (as a nested call from `name`'s
    /// running script within `execution_id`) and logging each call to
    /// `self.logger`.
    ///
    /// # Errors
    fn install_api_binding(&self, lua: &Lua, name: &str, execution_id: &str) -> error::Result<()> {
        let engine = Arc::clone(&self.engine);
        let logger = self.logger.clone();
        let name = name.to_owned();
        let execution_id = execution_id.to_owned();

        let run_fn = lua.create_function(
            move |lua, (id, params, options): (String, mlua::Value, Option<mlua::Value>)| {
                let now = chrono::offset::Local::now();
                let now = now.format(constants::DATETIME_FORMAT).to_string();

                logger
                    .write_all(format!("{now} ({name}) [API] {id}\n").as_bytes())
                    .map_err(|err| mlua::Error::ExternalError(Arc::new(err)))?;

                let params: serde_json::Value = lua.from_value(params)?;
                let options: serde_json::Value = options
                    .map(|value| lua.from_value(value))
                    .transpose()?
                    .unwrap_or(serde_json::Value::Null);

                let context =
                    EngineInputContext::new(Some(name.clone()), execution_id.clone(), false);

                let engine = engine.read().map_err(|err| {
                    mlua::Error::ExternalError(Arc::new(error::LuaActionRunner::PoisonedLock(
                        err.to_string(),
                    )))
                })?;
                let result = engine
                    .run(&id, params, options, &context)
                    .map_err(|err| mlua::Error::ExternalError(Arc::new(err)))?;

                lua.to_value(&result)
            },
        )?;

        let api = lua.create_table()?;
        api.set("run", run_fn)?;
        lua.globals().set("api", api)?;

        Ok(())
    }

    /// Wraps `source_code` via `local input = ...`, evaluates it in a
    /// fresh sandboxed interpreter (with `params` bound as `input` and
    /// the `api.run` binding installed), and converts the return value
    /// back to JSON.
    fn run_internal(
        &self,
        name: &str,
        _operation_name: &str,
        source_code: &str,
        params: &serde_json::Value,
        ctx: &EngineInputContext,
    ) -> error::Result<serde_json::Value> {
        let lua = Self::sandboxed_lua()?;
        self.install_api_binding(&lua, name, &ctx.execution_id)?;

        let input = lua.to_value(params)?;
        let wrapped = format!("local input = ...\n{source_code}");
        let func: mlua::Function = lua.load(&wrapped).into_function()?;
        let result: mlua::Value = func.call(input)?;

        lua.from_value(result)
            .map_err(|err| error::LuaActionRunner::ConversionError(err.to_string()))
    }
}

impl CodeRunner for LuaActionRunner {
    #[inline]
    fn run(
        &self,
        name: &str,
        operation_name: &str,
        source_code: &str,
        params: serde_json::Value,
        ctx: &EngineInputContext,
    ) -> execution_engine::error::Result<serde_json::Value> {
        let result = self.run_internal(name, operation_name, source_code, &params, ctx)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use execution_engine::services::EngineLookup;
    use serde_json::json;

    use super::{error::LuaActionRunner, Arc, Duration, EngineInputContext, LogWriter, RwLock};

    struct FakeLookup;

    impl EngineLookup for FakeLookup {
        fn get_service(&self, _id: &str) -> Option<core_entities::service::VersionedServiceTree> {
            None
        }

        fn get_credentials(
            &self,
            _id: &str,
        ) -> Option<credential_entities::credentials::Authentication> {
            None
        }
    }

    fn test_runner() -> super::LuaActionRunner {
        let (logger, _handle) = LogWriter::spawn(tempfile::tempfile().unwrap());
        let lookup: Arc<Mutex<dyn EngineLookup + Send + Sync>> = Arc::new(Mutex::new(FakeLookup));
        let engine = Arc::new(RwLock::new(execution_engine::Engine::new(
            lookup,
            logger.clone(),
        )));

        super::LuaActionRunner::new(engine, logger)
    }

    fn test_ctx() -> EngineInputContext {
        EngineInputContext::new(None, "test-execution".to_owned(), false)
    }

    #[test]
    fn runs_a_simple_lua_script_and_round_trips_json() {
        let runner = test_runner();

        let result = runner
            .run_internal(
                "svc",
                "op",
                "return { greeting = 'hello ' .. input.name }",
                &json!({ "name": "world" }),
                &test_ctx(),
            )
            .unwrap();

        assert_eq!(result, json!({ "greeting": "hello world" }));
    }

    #[test]
    fn surfaces_a_lua_syntax_error_instead_of_panicking() {
        let runner = test_runner();

        let result = runner.run_internal(
            "svc",
            "op",
            "this is not valid lua (((",
            &json!({}),
            &test_ctx(),
        );

        assert!(
            matches!(result, Err(LuaActionRunner::LuaError(_))),
            "expected a LuaError, got {result:?}"
        );
    }

    #[test]
    fn surfaces_a_lua_runtime_error_instead_of_panicking() {
        let runner = test_runner();

        let result = runner.run_internal("svc", "op", "error('boom')", &json!({}), &test_ctx());

        assert!(
            matches!(result, Err(LuaActionRunner::LuaError(_))),
            "expected a LuaError, got {result:?}"
        );
    }

    #[test]
    fn api_run_binding_reaches_the_engine_and_surfaces_its_error() {
        let runner = test_runner();

        // FakeLookup never finds a service, so this proves api.run really
        // dispatches through the shared Engine (not a stub): the call
        // fails with a real NotFound error from Engine::run, caught here
        // by pcall rather than propagating out as a Rust-level error.
        let result = runner
            .run_internal(
                "svc",
                "op",
                "local ok = pcall(function() return api.run('other.op', {}) end)\n\
                 return { called = true, ok = ok }",
                &json!({}),
                &test_ctx(),
            )
            .unwrap();

        assert_eq!(result, json!({ "called": true, "ok": false }));
    }

    #[test]
    fn sandbox_has_no_os_or_io_access() {
        let runner = test_runner();

        let result = runner.run_internal("svc", "op", "return os.time()", &json!({}), &test_ctx());
        assert!(
            matches!(result, Err(LuaActionRunner::LuaError(_))),
            "expected calling os.time() to fail (os should not be loaded), got {result:?}"
        );

        let result = runner.run_internal(
            "svc",
            "op",
            "return io.open('/etc/hostname')",
            &json!({}),
            &test_ctx(),
        );
        assert!(
            matches!(result, Err(LuaActionRunner::LuaError(_))),
            "expected calling io.open(...) to fail (io should not be loaded), got {result:?}"
        );
    }

    #[test]
    fn sandbox_cannot_read_files_via_dofile_or_loadfile() {
        let runner = test_runner();

        let result = runner.run_internal(
            "svc",
            "op",
            "return dofile('/etc/hostname')",
            &json!({}),
            &test_ctx(),
        );
        assert!(
            matches!(result, Err(LuaActionRunner::LuaError(_))),
            "expected dofile to be removed from the sandbox, got {result:?}"
        );

        let result = runner.run_internal(
            "svc",
            "op",
            "return loadfile('/etc/hostname')",
            &json!({}),
            &test_ctx(),
        );
        assert!(
            matches!(result, Err(LuaActionRunner::LuaError(_))),
            "expected loadfile to be removed from the sandbox, got {result:?}"
        );
    }

    #[test]
    fn a_runaway_script_is_aborted_after_its_time_budget() {
        let start = std::time::Instant::now();

        let lua =
            super::LuaActionRunner::sandboxed_lua_with_timeout(Duration::from_millis(50)).unwrap();
        let result: mlua::Result<()> = lua.load("while true do end").exec();

        assert!(result.is_err(), "expected the runaway script to error out");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "expected the script to be aborted well before its actual runtime would end, took {:?}",
            start.elapsed()
        );
    }
}
