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

//! A [`CodeRunner`] adapter that executes a Lua operation body in a fresh
//! embedded [`mlua::Lua`] interpreter per call.
//!
//! Unlike `python_runner`/`javascript_runner`, this runner has no
//! `api`/`workflow`/`action`/`task`-style bindings for calling back into
//! the shared [`execution_engine::Engine`] yet — that's deliberately
//! deferred to a follow-up pass that also decides this runner's
//! sandboxing policy (which parts of Lua's standard library a script can
//! reach, and what execution-time budget it's given). For now a Lua step
//! is a pure function of its `params`.
//!
//! Constructing a fresh `Lua` interpreter per call is cheap enough
//! (measured ~60-150µs, no one-time boot cost) that — unlike
//! `python_runner`'s process-wide interpreter or `javascript_runner`'s
//! thread-local `MiniV8` — no reuse strategy is needed here.

pub mod error;

use execution_engine::services::CodeRunner;
use mlua::{Lua, LuaSerdeExt};

/// Runs `source_code` as a Lua chunk with `params` bound as its local
/// `input` argument (via `local input = ...`), and converts the chunk's
/// return value back to JSON.
///
/// # Errors
fn run_lua(source_code: &str, params: &serde_json::Value) -> error::Result<serde_json::Value> {
    let lua = Lua::new();

    let input = lua.to_value(params)?;
    let wrapped = format!("local input = ...\n{source_code}");
    let func: mlua::Function = lua.load(&wrapped).into_function()?;
    let result: mlua::Value = func.call(input)?;

    lua.from_value(result)
        .map_err(|err| error::LuaActionRunner::ConversionError(err.to_string()))
}

/// A [`CodeRunner`] that executes a Lua operation body as a pure function
/// of its `params` (see the crate-level docs for what's deliberately not
/// supported yet).
#[derive(Default)]
pub struct LuaActionRunner;

impl LuaActionRunner {
    /// Creates a [`LuaActionRunner`].
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self
    }
}

impl CodeRunner for LuaActionRunner {
    #[inline]
    fn run(
        &self,
        _name: &str,
        _operation_name: &str,
        source_code: &str,
        params: serde_json::Value,
        _ctx: &execution_engine::services::EngineInputContext,
    ) -> execution_engine::error::Result<serde_json::Value> {
        let result = run_lua(source_code, &params)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{error::LuaActionRunner, run_lua};

    #[test]
    fn runs_a_simple_lua_script_and_round_trips_json() {
        let result = run_lua(
            "return { greeting = 'hello ' .. input.name }",
            &json!({ "name": "world" }),
        )
        .unwrap();

        assert_eq!(result, json!({ "greeting": "hello world" }));
    }

    #[test]
    fn surfaces_a_lua_syntax_error_instead_of_panicking() {
        let result = run_lua("this is not valid lua (((", &json!({}));

        assert!(
            matches!(result, Err(LuaActionRunner::LuaError(_))),
            "expected a LuaError, got {result:?}"
        );
    }

    #[test]
    fn surfaces_a_lua_runtime_error_instead_of_panicking() {
        let result = run_lua("error('boom')", &json!({}));

        assert!(
            matches!(result, Err(LuaActionRunner::LuaError(_))),
            "expected a LuaError, got {result:?}"
        );
    }
}
