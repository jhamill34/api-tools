//! A [`CodeRunner`] adapter that executes a JavaScript operation body
//! inside a [`MiniV8`] interpreter reused per thread.

// pub mod bindings;
mod constants;
mod converters;
pub mod error;

use mini_v8::MiniV8;

use std::{
    cell::RefCell,
    sync::{Arc, LazyLock},
};

use common_data_structures::log_writer::LogWriter;
use core_entities::ports::engine::{self, CodeRunner, EngineInputContext, EngineService};
use core_json_compat::{from_json, to_json};

use regex::{Captures, Regex};

static ARROW_FUNC: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?P<line>\(\s*\w*\s*\)\s*=>\s*)").ok());
static REGULAR_FUNC: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"function\s*(?P<name>\w+)\s*\(\s*\w*\s*\)\s*").ok());

thread_local! {
    /// This thread's cached `MiniV8` instance, lazily created on first use.
    static THREAD_MINI_V8: RefCell<Option<MiniV8>> = const { RefCell::new(None) };
}

/// Returns this thread's cached [`MiniV8`] instance, creating one on first
/// use. Constructing a `MiniV8` builds a fresh V8 isolate, which costs
/// low-single-digit milliseconds — reusing one per thread instead of per
/// call avoids paying that cost on every JavaScript step. `MiniV8` isn't
/// `Send` (it wraps `Rc`-based state internally), so instances can't be
/// shared across threads; caching one per thread is the safe way to reuse
/// them anyway, since `apid` dispatches each step onto a pool of reused
/// worker threads via `tokio::task::spawn_blocking`.
///
/// Reusing the isolate means its global object persists across calls on
/// the same thread — a script that assigns onto `globalThis` (rather than
/// declaring a local with `var`/`let`/`const`, which every wrapped script
/// body already does per [`wrap_source_code`]) would see that state on a
/// later, unrelated call on the same thread. This mirrors the tradeoff
/// already accepted for `python_runner`'s process-wide interpreter reuse.
fn thread_mini_v8() -> MiniV8 {
    THREAD_MINI_V8.with(|cell| cell.borrow_mut().get_or_insert_with(MiniV8::new).clone())
}

/// Rewrites a source body whose top-level function is an arrow function
/// (matched by `re`) into a `(input, api) => { ... }` wrapper that calls it.
fn handle_arrow_func(source: &str, re: &Regex) -> String {
    let source = re.replace(source, "const __internal_arrow = $line");

    format!("(input, api) => {{\n\n{source}\n\n; return __internal_arrow(input);\n\n}}\n\n")
}

/// Rewrites a source body whose top-level function is a named `function`
/// declaration into a `(input, api) => { ... }` wrapper that calls it by
/// the name captured in `captures`.
fn handle_regular_func(source: &str, captures: &Captures) -> error::Result<String> {
    if let Some(name) = captures.name("name") {
        let name = name.as_str();
        Ok(format!(
            "(input, api) => {{\n\n{source}\n\n; return {name}(input);\n\n}}\n\n"
        ))
    } else {
        Err(error::JsActionRunner::NoFunctionFound(
            "Arrow Function".into(),
        ))
    }
}

/// Detects whether `source`'s top-level function is an arrow function or a
/// named `function` declaration, and wraps it in a
/// `(input, api) => { ... }` shim so it can be invoked uniformly.
fn wrap_source_code(source: &str) -> error::Result<String> {
    if let Some(arrow_func) = ARROW_FUNC.as_ref() {
        if arrow_func.is_match(source) {
            return Ok(handle_arrow_func(source, arrow_func));
        }
    }

    if let Some(regular_func) = REGULAR_FUNC.as_ref() {
        if let Some(captures) = regular_func.captures(source) {
            return handle_regular_func(source, &captures);
        }
    }

    Err(error::JsActionRunner::NoFunctionFound(
        "No Regular or Arrow Function Found".into(),
    ))
}

/// A [`CodeRunner`] that wraps and executes a JavaScript operation body in
/// this thread's cached `MiniV8` interpreter (see [`thread_mini_v8`]),
/// exposing an `api.run(id, params)` binding the script can use to invoke
/// another operation on the shared [`EngineService`].
pub struct JsActionRunner {
    /// Where the `api.run` binding logs each nested call it makes.
    logger: LogWriter,

    /// The engine used to resolve `api.run` calls made from JavaScript.
    engine: Arc<dyn EngineService>,
}

impl JsActionRunner {
    /// Creates a [`JsActionRunner`] that dispatches nested calls through
    /// `engine` and logs them to `logger`.
    #[inline]
    pub fn new(engine: Arc<dyn EngineService>, logger: LogWriter) -> Self {
        Self { logger, engine }
    }

    /// Wraps `source_code` via [`wrap_source_code`], evaluates it in this
    /// thread's cached `MiniV8` interpreter with `params` as input and an
    /// `api.run` binding installed, and converts the JavaScript return
    /// value back to JSON.
    fn run_internal(
        &self,
        name: &str,
        _operation_name: &str,
        source_code: &str,
        params: serde_json::Value,
        ctx: &EngineInputContext,
    ) -> error::Result<serde_json::Value> {
        let mv8 = thread_mini_v8();

        let logger = self.logger.clone();
        let engine = Arc::clone(&self.engine);
        let name = name.to_owned();
        let execution_id = ctx.execution_id.clone();
        let api_binding = mv8.create_function(move |inv| -> mini_v8::Result<mini_v8::Value> {
            let (id, params, options): (String, mini_v8::Value, Option<mini_v8::Value>) =
                inv.args.into(&inv.mv8)?;

            let now = chrono::offset::Local::now();
            let now = now.format(constants::DATETIME_FORMAT).to_string();

            logger
                .write_all(format!("{now} ({}) [API] {id}\n", name.clone()).as_bytes())
                .map_err(|err| mini_v8::Error::ExternalError(Box::new(err)))?;

            let params = converters::from_v8(params)?;
            let options = if let Some(options) = options {
                converters::from_v8(options)?
            } else {
                serde_json::Value::Null
            };

            let context = EngineInputContext::new(Some(name.clone()), execution_id.clone(), false);
            let result = engine
                .run(&id, from_json(params), from_json(options), &context)
                .map_err(|err| mini_v8::Error::ExternalError(Box::new(err)))?;

            let output = converters::from_value(&inv.mv8, to_json(result))?;

            Ok(output)
        });
        let api = mv8.create_object();
        api.set("run", api_binding)
            .map_err(|err| error::JsActionRunner::V8(err.to_string()))?;

        let source_code = wrap_source_code(source_code)?;

        let execute_internal: mini_v8::Function = mv8
            .eval(source_code)
            .map_err(|err| error::JsActionRunner::V8(err.to_string()))?;

        let inputs = converters::from_value(&mv8, params)
            .map_err(|err| error::JsActionRunner::V8(err.to_string()))?;

        let output = execute_internal
            .call((inputs, api))
            .map_err(|err| error::JsActionRunner::V8(err.to_string()))?;

        let result = converters::from_v8(output)
            .map_err(|err| error::JsActionRunner::V8(err.to_string()))?;

        Ok(result)
    }
}

impl CodeRunner for JsActionRunner {
    #[inline]
    fn run(
        &self,
        name: &str,
        operation_name: &str,
        source_code: &str,
        params: engine::RuntimeValue,
        ctx: &EngineInputContext,
    ) -> engine::error::Result<engine::RuntimeValue> {
        let result = self.run_internal(name, operation_name, source_code, to_json(params), ctx)?;
        Ok(from_json(result))
    }
}

#[cfg(test)]
mod tests {
    use super::thread_mini_v8;

    #[test]
    fn thread_mini_v8_reuses_the_same_isolate_across_calls() {
        let first = thread_mini_v8();
        first.eval::<_, ()>("globalThis.__probe = 42;").unwrap();

        let second = thread_mini_v8();
        let value: i32 = second.eval("globalThis.__probe").unwrap();

        assert_eq!(
            value, 42,
            "expected the second call to see state set by the first, proving the isolate was reused"
        );
    }
}
