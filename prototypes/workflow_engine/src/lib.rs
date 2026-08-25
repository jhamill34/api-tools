//! Spike for issue #68: a coroutine-based, `mlua`-backed engine for
//! user-defined decision workflows. See `README.md` for scope.
//!
//! Core mechanism: `api.step(fn)` wraps a Lua function as a lazily-started,
//! memoized async step (a [`StepHandle`]). Calling `handle:get()` on one
//! handle runs (and caches) that single step. Real concurrency comes from
//! `api.join({h1, h2, ...})`, which awaits several handles' underlying
//! futures together via [`futures::future::join_all`] - awaiting handles
//! one at a time via repeated `:get()` stays sequential, same as a plain
//! blocking call would (verified empirically before this was built; see
//! the research-findings comment on #68).
//!
//! **A load-bearing subtlety, found the hard way (a genuinely hung test,
//! not a guess):** `mlua`'s execution-time hooks are per-coroutine, not
//! per-`Lua`-instance. `Function::call_async` internally creates its own
//! fresh, un-hooked coroutine for every call (`create_recycled_thread`) -
//! a hook set once on the parent [`Lua`] at construction time never runs
//! inside it. Worse, a stuck coroutine like that doesn't just miss its
//! timeout: `mlua::Thread::poll`'s call into Lua's C API doesn't yield
//! back to the async executor at all until *something* (a hook or a
//! genuine yield point) interrupts it, so even wrapping the call in
//! `tokio::time::timeout` does not help - the outer timer future never
//! gets polled, because the inner poll() call itself never returns. Every
//! call site here creates its coroutine explicitly via
//! [`call_hooked_async`] instead of `Function::call_async`, so it can set
//! a fresh hook on that specific thread before running it.

pub mod error;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use error::WorkflowError;
use mlua::{
    Function, HookTriggers, IntoLuaMulti, Lua, LuaSerdeExt, RegistryKey, StdLib, UserData, Value,
};
use tokio::sync::OnceCell;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const HOOK_INSTRUCTION_INTERVAL: u32 = 1000;

/// Creates a fresh coroutine from `func`, arms it with its own
/// `timeout`-based hook (re-armed from `Instant::now()` at this call, not
/// shared across calls - see the module-level note on why every
/// `call_async`-equivalent needs its own hooked thread), and awaits it.
async fn call_hooked_async<'lua, A, R>(
    lua: &'lua Lua,
    func: Function<'lua>,
    args: A,
    timeout: Duration,
) -> mlua::Result<R>
where
    A: IntoLuaMulti<'lua>,
    R: mlua::FromLuaMulti<'lua>,
{
    let thread = lua.create_thread(func)?;
    let start = Instant::now();
    thread.set_hook(
        HookTriggers::default().every_nth_instruction(HOOK_INSTRUCTION_INTERVAL),
        move |_lua, _debug| {
            if start.elapsed() > timeout {
                return Err(mlua::Error::RuntimeError(
                    "workflow exceeded its execution time budget".into(),
                ));
            }
            Ok(())
        },
    );
    thread.into_async(args).await
}

/// A sandboxed Lua VM with the `api.step`/`api.join` workflow bindings
/// installed. Host functions (e.g. a real `db_lookup`) are registered
/// separately via [`WorkflowEngine::register_async_function`] - this
/// crate has no built-in host functions of its own.
pub struct WorkflowEngine {
    lua: Lua,
    timeout: Duration,
}

impl WorkflowEngine {
    /// Builds a sandboxed engine with [`DEFAULT_TIMEOUT`] and no memory
    /// limit.
    ///
    /// # Errors
    /// Returns an error if the underlying Lua VM or its bindings fail to
    /// construct.
    pub fn new() -> error::Result<Self> {
        Self::with_limits(DEFAULT_TIMEOUT, None)
    }

    /// Builds a sandboxed engine with an explicit wall-clock `timeout`
    /// (applied fresh to every script/step coroutine it runs - see the
    /// module docs) and an optional `memory_limit` (bytes, enforced by
    /// `mlua`'s own allocator hook).
    ///
    /// # Errors
    /// Returns an error if the underlying Lua VM or its bindings fail to
    /// construct.
    pub fn with_limits(timeout: Duration, memory_limit: Option<usize>) -> error::Result<Self> {
        let lua = Lua::new_with(
            StdLib::TABLE | StdLib::STRING | StdLib::MATH,
            mlua::LuaOptions::default(),
        )?;

        // mlua loads Lua's base library unconditionally regardless of the
        // `StdLib` selection above, and base includes `dofile`/`loadfile`,
        // which read arbitrary files from disk - close that explicitly
        // (same gap found and fixed in runners/lua_runner, #61).
        lua.globals().set("dofile", Value::Nil)?;
        lua.globals().set("loadfile", Value::Nil)?;

        if let Some(limit) = memory_limit {
            lua.set_memory_limit(limit)?;
        }

        install_step_api(&lua, timeout)?;

        Ok(Self { lua, timeout })
    }

    /// Registers `func` as an async Lua-callable global named `name` (e.g.
    /// a real or mocked `db_lookup`). `func` receives JSON-ish `mlua`
    /// [`Value`] arguments and must return an `mlua::Result<Value>` future.
    ///
    /// # Errors
    /// Returns an error if the underlying Lua call to register the
    /// function fails.
    pub fn register_async_function<'lua, F, FR>(
        &'lua self,
        name: &str,
        func: F,
    ) -> error::Result<()>
    where
        F: Fn(&'lua Lua, mlua::MultiValue<'lua>) -> FR + Send + 'static,
        FR: std::future::Future<Output = mlua::Result<Value<'lua>>> + 'lua,
    {
        let f = self.lua.create_async_function(func)?;
        self.lua.globals().set(name, f)?;
        Ok(())
    }

    /// Loads and runs `script` as the workflow's entry point, returning its
    /// JSON-converted return value.
    ///
    /// # Errors
    /// Returns an error if the script fails to parse, errors at runtime,
    /// hits its timeout/memory budget, or its return value can't be
    /// converted to JSON.
    pub async fn run(&self, script: &str) -> error::Result<serde_json::Value> {
        let func: Function = self.lua.load(script).into_function()?;
        let result: Value = call_hooked_async(&self.lua, func, (), self.timeout).await?;
        self.lua
            .from_value(result)
            .map_err(|err| WorkflowError::Conversion(err.to_string()))
    }
}

/// A handle to a lazily-started, memoized workflow step, created by
/// `api.step(fn)`. `Clone`able since both `:get()` (via the `UserData`
/// wrapper) and `api.join` need independent, cheap access to the same
/// underlying cache.
#[derive(Clone)]
struct StepHandle {
    lua_fn_key: Arc<RegistryKey>,
    cache: Arc<OnceCell<Result<serde_json::Value, WorkflowError>>>,
    timeout: Duration,
}

impl StepHandle {
    /// Runs the wrapped function on first call (via [`call_hooked_async`],
    /// so it doesn't block a thread while awaiting and still gets its own
    /// timeout budget) and caches the outcome; later calls (including
    /// concurrent ones, via [`OnceCell`]'s own synchronization) return the
    /// cached result without re-running it.
    async fn resolve(&self, lua: &Lua) -> Result<serde_json::Value, WorkflowError> {
        self.cache
            .get_or_init(|| async {
                let func: Function = lua.registry_value(&self.lua_fn_key)?;
                let result: Value = call_hooked_async(lua, func, (), self.timeout).await?;
                lua.from_value(result)
                    .map_err(|err| WorkflowError::Conversion(err.to_string()))
            })
            .await
            .clone()
    }
}

impl UserData for StepHandle {
    fn add_methods<'lua, M: mlua::UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_async_method("get", |lua, this, ()| async move {
            let value = this.resolve(lua).await?;
            lua.to_value(&value)
        });
    }
}

fn install_step_api(lua: &Lua, timeout: Duration) -> error::Result<()> {
    let api = lua.create_table()?;

    let step_fn = lua.create_function(move |lua, f: Function| {
        let key = lua.create_registry_value(f)?;
        Ok(StepHandle {
            lua_fn_key: Arc::new(key),
            cache: Arc::new(OnceCell::new()),
            timeout,
        })
    })?;
    api.set("step", step_fn)?;

    let join_fn = lua.create_async_function(|lua, handles: mlua::Table| async move {
        let mut resolved = Vec::new();
        for pair in handles.sequence_values::<mlua::AnyUserData>() {
            let ud = pair?;
            let handle = ud.borrow::<StepHandle>()?.clone();
            resolved.push(handle);
        }

        let futs = resolved.iter().map(|h| h.resolve(lua));
        let results = futures::future::join_all(futs).await;

        let out = lua.create_table()?;
        for (i, result) in results.into_iter().enumerate() {
            let value = result?;
            out.set(i + 1, lua.to_value(&value)?)?;
        }
        Ok(out)
    })?;
    api.set("join", join_fn)?;

    lua.globals().set("api", api)?;

    Ok(())
}
