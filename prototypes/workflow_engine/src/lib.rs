//! Spike for issue #68: a coroutine-based, `mlua`-backed engine for
//! user-defined decision workflows. See `README.md` for scope.
//!
//! Core mechanism: `api.step(fn)` wraps a Lua function as a memoized async
//! step (a [`StepHandle`]). Two things happen when it's called:
//!
//! 1. It's registered with [`WorkflowEngine::run`]'s driver loop, which
//!    starts making real progress on it - *without* the script ever
//!    awaiting it - the next time the script's own execution yields (e.g.
//!    while awaiting its own direct async call). This is the actual
//!    automatic/eager scheduling `api.step` provides.
//! 2. Calling `handle:get()` later runs it (if the driver hasn't already)
//!    and caches the outcome; a second `:get()`, or the driver having
//!    already finished it, never re-runs the work.
//!
//! `api.join({h1, h2, ...})` still exists for explicitly awaiting several
//! handles together - it's mostly redundant with automatic eager
//! scheduling now, but remains a valid explicit synchronization point.
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
//!
//! **A second subtlety, also found empirically:** `tokio::select!` is the
//! wrong tool to drive the main script future alongside background steps.
//! `api.step`'s registration is a *synchronous side effect inside a poll
//! of the main future* - by the time `select!` finishes that poll and
//! loops back around to check for newly-registered steps, the main
//! script may already have finished (if it went on to directly await its
//! own work) without the background step ever having been created, let
//! alone polled. [`WorkflowEngine::run`] instead uses [`std::future::poll_fn`]
//! for manual control: drain newly-registered steps and give every
//! outstanding background step a chance to progress on *every single
//! poll* of the main future, not just between its Ready/Pending
//! transitions.
//!
//! **Known scoping limits, not yet solved here:** a step registered but
//! never awaited before the script returns is simply abandoned (dropped
//! mid-flight) - `run()` does not wait for outstanding background work to
//! finish, and an error from an abandoned step is silently lost. Calling
//! `run()` concurrently on the *same* [`WorkflowEngine`] instance is also
//! not safe - the per-run pending-step queue lives in Lua-instance-scoped
//! app data, shared across the whole engine, not per-call.

pub mod error;

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use error::WorkflowError;
use futures::{future::LocalBoxFuture, stream::FuturesUnordered, StreamExt};
use mlua::{
    Function, HookTriggers, IntoLuaMulti, Lua, LuaSerdeExt, RegistryKey, StdLib, UserData, Value,
};
use tokio::sync::OnceCell;

type StepCache = Arc<OnceCell<Result<serde_json::Value, WorkflowError>>>;

/// Steps registered via `api.step(fn)`, drained and driven concurrently by
/// [`WorkflowEngine::run`]'s poll loop. Installed fresh (as Lua app data)
/// at the start of every `run()` call - see the module docs' "known
/// scoping limits" note on why concurrent `run()` calls on one engine
/// aren't safe yet.
type PendingQueue = Arc<Mutex<Vec<(Arc<RegistryKey>, StepCache)>>>;

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

    /// Loads and runs `script` as the workflow's entry point with `params`
    /// bound as a local `input` argument (`local input = ...`, the same
    /// chunk-argument convention `runners/lua_runner` uses), returning its
    /// JSON-converted return value.
    ///
    /// Drives the script's own coroutine and any `api.step`-registered
    /// background steps together via a hand-rolled [`std::future::poll_fn`]
    /// loop (see the module docs for why `tokio::select!` doesn't work
    /// here), so steps registered but not yet awaited still make real
    /// progress whenever the script itself yields.
    ///
    /// # Errors
    /// Returns an error if the script fails to parse, errors at runtime,
    /// hits its timeout/memory budget, or its return value (or `params`)
    /// can't be converted between JSON and Lua.
    pub async fn run(
        &self,
        script: &str,
        params: serde_json::Value,
    ) -> error::Result<serde_json::Value> {
        let lua = &self.lua;
        let timeout = self.timeout;

        let pending: PendingQueue = Arc::new(Mutex::new(Vec::new()));
        lua.set_app_data(pending.clone());

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
        let mut main_fut = thread.into_async::<_, Value>(input);

        let mut background: FuturesUnordered<LocalBoxFuture<'_, ()>> = FuturesUnordered::new();
        let mut known = 0usize;

        let result: Value = std::future::poll_fn(|cx| {
            use std::future::Future;

            if let std::task::Poll::Ready(result) = std::pin::Pin::new(&mut main_fut).poll(cx) {
                return std::task::Poll::Ready(result);
            }

            {
                let queue = pending
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                while known < queue.len() {
                    let (key, cache) = queue[known].clone();
                    known += 1;
                    background.push(build_step_future(lua, key, cache, timeout));
                }
            }

            while let std::task::Poll::Ready(Some(())) = background.poll_next_unpin(cx) {}

            std::task::Poll::Pending
        })
        .await?;

        lua.from_value(result)
            .map_err(|err| WorkflowError::Conversion(err.to_string()))
    }
}

/// Builds the future that actually runs (and caches) a registered step,
/// used both by [`WorkflowEngine::run`]'s eager driver and by
/// [`StepHandle::resolve`] - whichever gets there first via
/// [`OnceCell::get_or_init`] runs the work; the other just awaits the same
/// cached outcome.
fn build_step_future<'lua>(
    lua: &'lua Lua,
    key: Arc<RegistryKey>,
    cache: StepCache,
    timeout: Duration,
) -> LocalBoxFuture<'lua, ()> {
    Box::pin(async move {
        let _ = cache
            .get_or_init(|| async {
                let func: Function = lua.registry_value(&key)?;
                let result: Value = call_hooked_async(lua, func, (), timeout).await?;
                lua.from_value(result)
                    .map_err(|err| WorkflowError::Conversion(err.to_string()))
            })
            .await;
    })
}

/// A handle to a lazily-started, memoized workflow step, created by
/// `api.step(fn)`. `Clone`able since both `:get()` (via the `UserData`
/// wrapper) and `api.join` need independent, cheap access to the same
/// underlying cache.
#[derive(Clone)]
struct StepHandle {
    lua_fn_key: Arc<RegistryKey>,
    cache: StepCache,
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
        let key = Arc::new(lua.create_registry_value(f)?);
        let cache: StepCache = Arc::new(OnceCell::new());

        // Register with the current run's driver loop (see
        // `WorkflowEngine::run`) so this step starts making real progress
        // the next time the script itself yields, whether or not the
        // script ever calls `:get()` on the handle returned here.
        let pending = lua.app_data_ref::<PendingQueue>().ok_or_else(|| {
            mlua::Error::RuntimeError(
                "api.step called outside of WorkflowEngine::run - no pending-step queue installed"
                    .into(),
            )
        })?;
        pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((Arc::clone(&key), Arc::clone(&cache)));

        Ok(StepHandle {
            lua_fn_key: key,
            cache,
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
