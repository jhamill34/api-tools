# workflow_engine (prototype)

Spike implementation for [issue #68](https://github.com/jhamill34/api-tools/issues/68):
a coroutine-based, mlua-backed scripting engine for user-defined decision
workflows.

**Status: prototype.** Not wired into `apid`/`execution_engine`. Nothing in
this crate is called from production code yet.

## What this slice proves

- `api.step(fn)` returns a handle wrapping an async call into a host
  function (`db_lookup` in tests/examples here is a mock). The step is
  registered with `WorkflowEngine::run`'s driver loop immediately, and
  starts making real progress the next time the script's own execution
  yields (e.g. while it directly awaits its own separate call) — *without*
  the script ever calling `:get()` on it. This is real, automatic eager
  scheduling (the "`FuturesUnordered`-style driver" language from #68),
  not just an explicit combinator — see
  `a_step_registered_but_never_get_before_the_scripts_own_direct_call_still_progresses_eagerly`
  in `tests/workflow.rs`.
- `handle:get()` runs the step (if the driver hasn't already) and memoizes
  the result — a second `:get()`, or the driver having already finished it
  first, never re-runs the underlying work.
- `api.join({h1, h2, ...})` still explicitly awaits multiple handles
  together — now mostly redundant with automatic eager scheduling for pure
  concurrency, but still a valid explicit synchronization point.
- A wall-clock execution timeout and an `mlua` memory limit both abort a
  runaway script instead of hanging or exhausting host memory.

## How eager scheduling actually works (and why `tokio::select!` doesn't)

`mlua`'s registered callbacks (`create_function`/`create_async_function`)
must be `'static` — they can't capture a channel or future tied to the
`Lua` instance's own borrowed lifetime. So `api.step(fn)` doesn't build or
send a future at all; it pushes a `(RegistryKey, cache)` pair into a
per-run queue stored as Lua app data (both `'static`-compatible types),
and `WorkflowEngine::run`'s driver — which legitimately holds a live
`&Lua` — is the thing that actually constructs and polls the step's
coroutine.

That driver can't be `tokio::select!` between the main script future and a
`FuturesUnordered` of steps, either: `api.step`'s queue push is a
*synchronous side effect inside a poll* of the main future. By the time
`select!` finishes that poll and loops back to check for new steps, the
main script may have already directly awaited its own work and returned —
the step was registered too late to matter. `run()` instead uses
`std::future::poll_fn` for manual control, draining the queue and giving
every outstanding background step a chance to progress on *every single
poll* of the main future, not just between its Ready/Pending transitions.
Both of the above were found empirically (a genuinely-too-slow test in the
`select!` case), not assumed.

## Known scoping limits, not solved by this slice

- A step registered but never awaited before the script returns is simply
  abandoned mid-flight — `run()` doesn't wait for outstanding background
  work to finish, and an error from an abandoned step is silently lost.
- Calling `run()` concurrently on the *same* `WorkflowEngine` instance
  isn't safe — the per-run pending-step queue lives in Lua-instance-scoped
  app data, shared across the whole engine, not scoped per call.
- `mlua`'s `send` Cargo feature is enabled here, but `Thread`/`AsyncThread`
  values themselves are not `Send` — confirmed empirically while building
  this slice (a channel-based first attempt at eager scheduling didn't
  compile for exactly this reason, which is why the design moved to the
  app-data/`RegistryKey` approach described above instead). Directly
  answers #68's own flagged open question ("send feature requirements...
  confirm compatibility") for this specific mechanism.

## Deliberately out of scope for this slice

- Tier 2 (`api.call_workflow` / isolated child VMs) and Tier 3 (Temporal)
  from #68 — separate, later slices.
- Any wiring into `execution_engine`/`apid` — `api_caller`/`Engine::run`
  are still fully synchronous (see #68's research-findings comment), so
  there's nothing real to call yet; `db_lookup` here stays a mock.
