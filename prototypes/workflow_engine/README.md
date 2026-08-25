# workflow_engine (prototype)

Spike implementation for [issue #68](https://github.com/jhamill34/api-tools/issues/68):
a coroutine-based, mlua-backed scripting engine for user-defined decision
workflows.

**Status: prototype.** Not wired into `apid`/`execution_engine`. Nothing in
this crate is called from production code yet.

## What this slice proves

- `api.step(fn)` returns a handle wrapping a not-yet-started async call into
  a host function (`db_lookup` in tests/examples here is a mock).
- `handle:get()` runs the step on first call and memoizes the result — a
  second `:get()` does not re-run the underlying work.
- `api.join({h1, h2, ...})` awaits multiple handles concurrently
  (`futures::future::join_all`), which is what actually delivers
  concurrency: awaiting handles one at a time via repeated `:get()` calls
  stays sequential, same as a plain blocking call would.
- A wall-clock execution timeout and an `mlua` memory limit both abort a
  runaway script instead of hanging or exhausting host memory.

## Deliberately out of scope for this slice

- Automatic, per-instruction eager scheduling (the "`FuturesUnordered`-style
  driver" language in #68) — that needs either `tokio::spawn` with a
  `'static`/`Send` Lua state, or a hand-rolled cooperative executor. This
  slice's concurrency is explicit (`api.join`), not automatic. Worth a
  follow-up once the explicit primitive is validated.
- Tier 2 (`api.call_workflow` / isolated child VMs) and Tier 3 (Temporal)
  from #68 — separate, later slices.
- Any wiring into `execution_engine`/`apid` — `api_caller`/`Engine::run`
  are still fully synchronous (see #68's research-findings comment), so
  there's nothing real to call yet; `db_lookup` here stays a mock.
