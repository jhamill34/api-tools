//! Spike for issue #68: a coroutine-based, `mlua`-backed engine for
//! user-defined decision workflows. See `README.md` for scope.
//!
//! Core mechanism (redesigned by issue #106 - see that issue for the
//! eager/implicit scheduling this replaced): `api.step(fn, deps)`
//! registers `fn` as a memoized, dependency-aware step (a [`StepHandle`]).
//! Registration is immediate and synchronous - subject to a `max_steps` cap
//! (`DEFAULT_MAX_STEPS`, a high backstop against pathological scripts, not
//! a limit meant to constrain normal usage - see issue #104 and
//! [`StepCount`]) - but nothing runs yet. `deps` is an optional list of
//! already-created `StepHandle`s; because a dependency must already exist
//! to be referenced, a script can never construct a dependency cycle - no
//! separate cycle-detection pass is needed.
//!
//! A step only actually executes once something *resolves* it:
//! `handle:get()`, being listed in `api.join({...})` or another step's
//! `deps`, or `api.terminal(...)` (see below). Resolving a step first
//! (concurrently) resolves its own `deps`; if any dependency failed, that
//! error short-circuits the step - its body never runs at all. Actually
//! running a step's body is separately gated by a semaphore
//! (`max_concurrent_steps`, see [`StepSemaphore`]) so a script that
//! resolves many steps at once doesn't spin up unbounded concurrent Lua
//! coroutines in one burst. Every resolution path funnels through the same
//! [`StepHandle::resolve`] - whichever caller reaches it first (via
//! [`OnceCell::get_or_init`]) does the work; everyone else just gets the
//! same cached outcome.
//!
//! `api.terminal(handle_or_handles)` declares the workflow's actual
//! output(s) - it resolves the reachable subgraph (concurrently, same
//! mechanism as `api.join`, which still exists for resolving a few handles
//! mid-script and continuing) and returns the resolved value(s) as a plain
//! Lua value/table, same as `:get()`/`api.join` already do - `api.terminal`
//! doesn't otherwise redirect the script's control flow, so the script is
//! still responsible for its own `return` (typically `return
//! api.terminal(...)` directly, but nothing requires that). Calling
//! `api.terminal` at least once is *required* if the script used
//! `api.step` at all - checked once the main script returns (see
//! [`WorkflowEngine::run`]) - which is what closes the old design's actual
//! gap: a step that's registered but never reached by any resolution path
//! (a dependent, `:get()`, `api.join`, or `api.terminal`) simply never
//! runs at all, rather than starting in the background and then being
//! silently abandoned mid-flight if the script returned first.
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
//! gets polled, because the inner `poll()` call itself never returns. Every
//! call site here creates its coroutine explicitly via
//! [`call_hooked_async`] instead of `Function::call_async`, so it can set
//! a fresh hook on that specific thread before running it.
//!
//! **Known scoping limit, not yet solved here:** calling `run()`
//! concurrently on the *same* [`WorkflowEngine`] instance is not safe -
//! the per-run semaphore/step-count/terminal-flag state lives in
//! Lua-instance-scoped app data, shared across the whole engine, not
//! per-call.

pub mod error;

use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use error::WorkflowError;
use mlua::{
    Function, HookTriggers, IntoLuaMulti, Lua, LuaSerdeExt, RegistryKey, StdLib, UserData, Value,
};
use tokio::sync::{OnceCell, Semaphore};

type StepCache = Arc<OnceCell<Result<serde_json::Value, WorkflowError>>>;

/// Bounds how many `api.step` bodies may actually be *executing* at once -
/// not how many are registered, which is always immediate. Installed fresh
/// as Lua app data at the start of every `run()` call. Without this, a
/// script that resolves many steps at once (e.g. via one large
/// `api.terminal`/`api.join` call, or a long dependency fan-out) could spin
/// up unbounded concurrent Lua coroutines in one burst - see issue #102.
type StepSemaphore = Arc<Semaphore>;

/// Running total of steps registered via `api.step` in the current `run()`
/// call, checked against `max_steps` on every registration - this only
/// ever counts up. Installed fresh as Lua app data at the start of every
/// `run()` call, same as [`StepSemaphore`]. Also what [`WorkflowEngine::run`]
/// checks against [`TerminalFlag`] once the script returns, to enforce that
/// a script using `api.step` also called `api.terminal` - see issue #106.
type StepCount = Arc<AtomicUsize>;

/// Set once `api.terminal` is called, checked (alongside [`StepCount`])
/// once the main script returns to enforce that a script using `api.step`
/// also declared its output via `api.terminal` - see issue #106 and the
/// module docs. Installed fresh as Lua app data at the start of every
/// `run()` call, same as [`StepSemaphore`]/[`StepCount`].
type TerminalFlag = Arc<AtomicBool>;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const HOOK_INSTRUCTION_INTERVAL: u32 = 1000;
/// Default cap on concurrently-executing `api.step` bodies, used unless
/// [`WorkflowEngine::with_limits`] is given an explicit override.
const DEFAULT_MAX_CONCURRENT_STEPS: usize = 32;
/// Default cap on the *total* number of steps a single `run()` call may
/// register via `api.step`, used unless [`WorkflowEngine::with_limits`] is
/// given an explicit override. Deliberately much larger than
/// [`DEFAULT_MAX_CONCURRENT_STEPS`] - this isn't meant to constrain normal
/// usage in the low thousands, only to reject clearly pathological/runaway
/// scripts (tens of thousands of steps and up) outright - see issue #104.
const DEFAULT_MAX_STEPS: usize = 5_000;

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

/// A sandboxed Lua VM with the `api.step`/`api.join`/`api.terminal`
/// workflow bindings installed. Host functions (e.g. a real `db_lookup`)
/// are registered separately via [`WorkflowEngine::register_async_function`]
/// - this crate has no built-in host functions of its own.
pub struct WorkflowEngine {
    lua: Lua,
    timeout: Duration,
    max_concurrent_steps: usize,
}

impl WorkflowEngine {
    /// Builds a sandboxed engine with [`DEFAULT_TIMEOUT`] and no memory
    /// limit.
    ///
    /// # Errors
    /// Returns an error if the underlying Lua VM or its bindings fail to
    /// construct.
    pub fn new() -> error::Result<Self> {
        Self::with_limits(DEFAULT_TIMEOUT, None, None, None)
    }

    /// Builds a sandboxed engine with an explicit wall-clock `timeout`
    /// (applied fresh to every script/step coroutine it runs - see the
    /// module docs), an optional `memory_limit` (bytes, enforced by
    /// `mlua`'s own allocator hook), an optional `max_concurrent_steps` cap
    /// on how many `api.step` bodies may execute at once (`None` uses
    /// [`DEFAULT_MAX_CONCURRENT_STEPS`] - see [`StepSemaphore`]), and an
    /// optional `max_steps` cap on the *total* number of steps a single
    /// `run()` call may register (`None` uses [`DEFAULT_MAX_STEPS`] - see
    /// [`StepCount`]).
    ///
    /// # Errors
    /// Returns an error if the underlying Lua VM or its bindings fail to
    /// construct.
    pub fn with_limits(
        timeout: Duration,
        memory_limit: Option<usize>,
        max_concurrent_steps: Option<usize>,
        max_steps: Option<usize>,
    ) -> error::Result<Self> {
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

        install_step_api(&lua, timeout, max_steps.unwrap_or(DEFAULT_MAX_STEPS))?;

        Ok(Self {
            lua,
            timeout,
            max_concurrent_steps: max_concurrent_steps.unwrap_or(DEFAULT_MAX_CONCURRENT_STEPS),
        })
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

    /// Registers `func` as an async Lua-callable member of the workflow's
    /// `api` table (alongside `api.step`/`api.join`), named `name` - e.g.
    /// `api.run` for a host binding that dispatches back into an existing
    /// synchronous operation (see `apid`'s `WorkflowAdapter`). Unlike
    /// [`Self::register_async_function`], which installs a bare global,
    /// this nests the callable under `api.<name>` to match the workflow's
    /// existing `api.*` surface.
    ///
    /// # Errors
    /// Returns an error if the underlying Lua calls to look up `api` or
    /// register the function on it fail.
    pub fn register_api_function<'lua, F, FR>(&'lua self, name: &str, func: F) -> error::Result<()>
    where
        F: Fn(&'lua Lua, mlua::MultiValue<'lua>) -> FR + Send + 'static,
        FR: std::future::Future<Output = mlua::Result<Value<'lua>>> + 'lua,
    {
        let f = self.lua.create_async_function(func)?;
        let api: mlua::Table = self.lua.globals().get("api")?;
        api.set(name, f)?;
        Ok(())
    }

    /// Loads and runs `script` as the workflow's entry point with `params`
    /// bound as a local `input` argument (`local input = ...`, the same
    /// chunk-argument convention `runners/lua_runner` uses), returning its
    /// JSON-converted return value.
    ///
    /// Simply awaits the script's own coroutine to completion - steps only
    /// run when something resolves them (see the module docs), so there's
    /// no separate background driver to run alongside it. Once the script
    /// returns, enforces that `api.terminal` was called if the script used
    /// `api.step` at all - see [`error::WorkflowError::MissingTerminal`].
    ///
    /// # Errors
    /// Returns an error if the script fails to parse, errors at runtime,
    /// hits its timeout/memory budget, used `api.step` without ever calling
    /// `api.terminal`, or its return value (or `params`) can't be converted
    /// between JSON and Lua.
    pub async fn run(
        &self,
        script: &str,
        params: serde_json::Value,
    ) -> error::Result<serde_json::Value> {
        let lua = &self.lua;
        let timeout = self.timeout;

        let semaphore: StepSemaphore = Arc::new(Semaphore::new(self.max_concurrent_steps));
        lua.set_app_data(semaphore);

        let step_count: StepCount = Arc::new(AtomicUsize::new(0));
        lua.set_app_data(step_count.clone());

        let terminal_called: TerminalFlag = Arc::new(AtomicBool::new(false));
        lua.set_app_data(terminal_called.clone());

        let input = lua.to_value(&params)?;
        let wrapped = format!("local input = ...\n{script}");
        let func: Function = lua.load(&wrapped).into_function()?;
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

        let result: Value = thread.into_async::<_, Value>(input).await?;

        let registered = step_count.load(Ordering::Relaxed);
        if registered > 0 && !terminal_called.load(Ordering::Relaxed) {
            return Err(WorkflowError::MissingTerminal(registered));
        }

        lua.from_value(result)
            .map_err(|err| WorkflowError::Conversion(err.to_string()))
    }
}

/// A handle to a lazily-started, memoized workflow step, created by
/// `api.step(fn, deps)`. `Clone`able since `:get()` (via the `UserData`
/// wrapper), `api.join`, `api.terminal`, and any dependent step's own
/// `deps` list all need independent, cheap access to the same underlying
/// cache.
#[derive(Clone)]
struct StepHandle {
    lua_fn_key: Arc<RegistryKey>,
    cache: StepCache,
    timeout: Duration,
    /// Steps this one depends on, declared at registration time (`deps` in
    /// `api.step(fn, deps)`). Resolved (concurrently, memoized) before this
    /// step's own body runs - see [`Self::resolve`].
    deps: Vec<StepHandle>,
}

impl StepHandle {
    /// Resolves every dependency first (concurrently; short-circuits with
    /// the first dependency failure, as [`error::WorkflowError::DependencyFailed`],
    /// without ever running this step's body), then runs the wrapped
    /// function on first call (via [`call_hooked_async`], so it doesn't
    /// block a thread while awaiting and still gets its own timeout
    /// budget, gated by the run's [`StepSemaphore`]) and caches the
    /// outcome; later calls (including concurrent ones, via [`OnceCell`]'s
    /// own synchronization) return the cached result without re-running
    /// it. Every resolution path - `:get()`, `api.join`, `api.terminal`,
    /// or being another step's dependency - funnels through here.
    async fn resolve(&self, lua: &Lua) -> Result<serde_json::Value, WorkflowError> {
        self.cache
            .get_or_init(|| async {
                let dep_futs = self.deps.iter().map(|dep| dep.resolve(lua));
                for dep_result in futures::future::join_all(dep_futs).await {
                    dep_result.map_err(|err| WorkflowError::DependencyFailed(err.to_string()))?;
                }

                let semaphore = lua
                    .app_data_ref::<StepSemaphore>()
                    .expect("StepSemaphore installed fresh at the start of every run() call")
                    .clone();
                let _permit = semaphore
                    .acquire()
                    .await
                    .expect("the step semaphore is never closed");

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

/// Resolves every handle in `handles` concurrently (memoized, same as any
/// other resolution path - see [`StepHandle::resolve`]) and returns their
/// resolved values as a Lua sequence table in the same order. Shared by
/// `api.join` and `api.terminal`'s table form.
async fn resolve_table<'lua>(
    lua: &'lua Lua,
    handles: mlua::Table<'lua>,
) -> mlua::Result<mlua::Table<'lua>> {
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
}

fn install_step_api(lua: &Lua, timeout: Duration, max_steps: usize) -> error::Result<()> {
    let api = lua.create_table()?;

    let step_fn = lua.create_function(move |lua, (f, deps): (Function, Option<mlua::Table>)| {
        // Reject registration outright once the run's total step count
        // would exceed `max_steps` - checked (and counted) before this
        // step does anything else, so a rejected call leaves no trace (no
        // registry value, no dependency edges recorded). This is a
        // backstop against pathological/runaway scripts, not a limit meant
        // to bite during normal usage - see issue #104.
        let step_count = lua.app_data_ref::<StepCount>().ok_or_else(|| {
            mlua::Error::RuntimeError(
                "api.step called outside of WorkflowEngine::run - no step count installed".into(),
            )
        })?;
        let count = step_count.fetch_add(1, Ordering::Relaxed) + 1;
        if count > max_steps {
            return Err(mlua::Error::RuntimeError(format!(
                "workflow registered too many steps via api.step ({count} > {max_steps} max)"
            )));
        }
        drop(step_count);

        // `deps` can only name handles that already exist (they're `Lua`
        // values already bound to earlier `api.step` calls' return
        // values) - a script can never construct a forward reference, so
        // dependency cycles are impossible by construction. See the
        // module docs.
        let deps = match deps {
            Some(table) => {
                let mut out = Vec::new();
                for pair in table.sequence_values::<mlua::AnyUserData>() {
                    let ud = pair?;
                    out.push(ud.borrow::<StepHandle>()?.clone());
                }
                out
            }
            None => Vec::new(),
        };

        let key = Arc::new(lua.create_registry_value(f)?);
        let cache: StepCache = Arc::new(OnceCell::new());

        Ok(StepHandle {
            lua_fn_key: key,
            cache,
            timeout,
            deps,
        })
    })?;
    api.set("step", step_fn)?;

    let join_fn = lua.create_async_function(|lua, handles: mlua::Table| async move {
        resolve_table(lua, handles).await
    })?;
    api.set("join", join_fn)?;

    let terminal_fn = lua.create_async_function(|lua, arg: mlua::Value| async move {
        // Marks the run's output as declared - checked by
        // `WorkflowEngine::run` once the script returns, alongside
        // `StepCount`, to enforce that a script using `api.step` also
        // called `api.terminal` - see `TerminalFlag` and issue #106.
        let terminal_called = lua.app_data_ref::<TerminalFlag>().ok_or_else(|| {
            mlua::Error::RuntimeError(
                "api.terminal called outside of WorkflowEngine::run - no terminal flag installed"
                    .into(),
            )
        })?;
        terminal_called.store(true, Ordering::Relaxed);
        drop(terminal_called);

        match arg {
            mlua::Value::Table(handles) => {
                Ok(mlua::Value::Table(resolve_table(lua, handles).await?))
            }
            mlua::Value::UserData(ud) => {
                let handle = ud.borrow::<StepHandle>()?.clone();
                let value = handle.resolve(lua).await?;
                lua.to_value(&value)
            }
            other => Err(mlua::Error::RuntimeError(format!(
                "api.terminal expects a step handle or a table of step handles, got {}",
                other.type_name()
            ))),
        }
    })?;
    api.set("terminal", terminal_fn)?;

    lua.globals().set("api", api)?;

    Ok(())
}
