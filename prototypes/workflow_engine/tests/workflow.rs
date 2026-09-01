use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use workflow_engine::WorkflowEngine;

fn install_mock_db_lookup(engine: &WorkflowEngine, delay: Duration, call_count: Arc<AtomicUsize>) {
    engine
        .register_async_function("db_lookup", move |lua, args: mlua::MultiValue| {
            let call_count = Arc::clone(&call_count);
            let name: String = mlua::FromLuaMulti::from_lua_multi(args, lua).unwrap_or_default();
            async move {
                call_count.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(delay).await;
                Ok(mlua::Value::String(lua.create_string(&name)?))
            }
        })
        .expect("register db_lookup");
}

#[tokio::test]
async fn api_join_runs_independent_steps_concurrently() {
    let engine = WorkflowEngine::new().expect("build engine");
    install_mock_db_lookup(
        &engine,
        Duration::from_millis(150),
        Arc::new(AtomicUsize::new(0)),
    );

    let script = r#"
        local a = api.step(function() return db_lookup("A") end)
        local b = api.step(function() return db_lookup("B") end)
        local results = api.join({a, b})
        return { a = results[1], b = results[2] }
    "#;

    let start = Instant::now();
    let result = engine
        .run(script, serde_json::Value::Null)
        .await
        .expect("workflow run");
    let elapsed = start.elapsed();

    assert_eq!(result["a"], serde_json::json!("A"));
    assert_eq!(result["b"], serde_json::json!("B"));
    assert!(
        elapsed < Duration::from_millis(280),
        "expected the two steps to run concurrently (~150ms), took {elapsed:?}"
    );
}

#[tokio::test]
async fn step_get_memoizes_and_only_runs_the_underlying_call_once() {
    let engine = WorkflowEngine::new().expect("build engine");
    let call_count = Arc::new(AtomicUsize::new(0));
    install_mock_db_lookup(&engine, Duration::from_millis(10), Arc::clone(&call_count));

    let script = r#"
        local step = api.step(function() return db_lookup("A") end)
        local first = step:get()
        local second = step:get()
        return { first = first, second = second }
    "#;

    let result = engine
        .run(script, serde_json::Value::Null)
        .await
        .expect("workflow run");

    assert_eq!(result["first"], serde_json::json!("A"));
    assert_eq!(result["second"], serde_json::json!("A"));
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "expected the underlying db_lookup call to run exactly once, not once per :get()"
    );
}

#[tokio::test]
async fn sequential_get_calls_do_not_run_concurrently() {
    let engine = WorkflowEngine::new().expect("build engine");
    install_mock_db_lookup(
        &engine,
        Duration::from_millis(100),
        Arc::new(AtomicUsize::new(0)),
    );

    let script = r#"
        local a = api.step(function() return db_lookup("A") end):get()
        local b = api.step(function() return db_lookup("B") end):get()
        return { a = a, b = b }
    "#;

    let start = Instant::now();
    engine
        .run(script, serde_json::Value::Null)
        .await
        .expect("workflow run");
    let elapsed = start.elapsed();

    assert!(
        elapsed >= Duration::from_millis(190),
        "expected two sequential :get() calls to take ~200ms (sequential), took {elapsed:?} - \
         if this is fast, something is accidentally making plain :get() calls concurrent"
    );
}

#[tokio::test]
async fn a_runaway_script_is_aborted_after_its_time_budget() {
    let engine =
        WorkflowEngine::with_limits(Duration::from_millis(50), None, None).expect("build engine");

    let result = engine
        .run("while true do end", serde_json::Value::Null)
        .await;

    assert!(result.is_err(), "expected the runaway script to be aborted");
}

#[tokio::test]
async fn a_script_that_exceeds_its_memory_budget_is_aborted() {
    let engine = WorkflowEngine::with_limits(Duration::from_secs(5), Some(1024 * 1024), None)
        .expect("build engine");

    let script = r#"
        local t = {}
        for i = 1, 10000000 do
            t[i] = string.rep("x", 1000)
        end
        return "should not get here"
    "#;

    let result = engine.run(script, serde_json::Value::Null).await;

    assert!(
        result.is_err(),
        "expected the script exceeding its memory budget to be aborted"
    );
}

#[tokio::test]
async fn a_step_registered_but_never_get_before_the_scripts_own_direct_call_still_progresses_eagerly(
) {
    let engine = WorkflowEngine::new().expect("build engine");
    install_mock_db_lookup(
        &engine,
        Duration::from_millis(150),
        Arc::new(AtomicUsize::new(0)),
    );

    // "A" is registered via api.step but not retrieved until after the
    // script directly awaits its own "B" call. If A only starts once
    // something explicitly waits on it (the old, purely lazy behavior),
    // total time is ~300ms (B, then A, in series). If A starts eagerly -
    // making real progress while the script is separately suspended
    // awaiting B - total time is ~150ms (both concurrent).
    let script = r#"
        local step_a = api.step(function() return db_lookup("A") end)
        local b = db_lookup("B")
        local a = step_a:get()
        return { a = a, b = b }
    "#;

    let start = Instant::now();
    let result = engine
        .run(script, serde_json::Value::Null)
        .await
        .expect("workflow run");
    let elapsed = start.elapsed();

    assert_eq!(result["a"], serde_json::json!("A"));
    assert_eq!(result["b"], serde_json::json!("B"));
    assert!(
        elapsed < Duration::from_millis(280),
        "expected step A (registered eagerly, only retrieved after the script's own direct \
         call to B) to have already progressed concurrently with B, took {elapsed:?} - eager \
         scheduling isn't working, A only started once :get() was called"
    );
}

#[tokio::test]
async fn run_binds_params_as_a_local_input_argument() {
    let engine = WorkflowEngine::new().expect("build engine");

    let script = r#"
        return { greeting = "hello " .. input.name, doubled = input.value * 2 }
    "#;

    let result = engine
        .run(script, serde_json::json!({ "name": "world", "value": 21 }))
        .await
        .expect("workflow run");

    assert_eq!(
        result,
        serde_json::json!({ "greeting": "hello world", "doubled": 42 })
    );
}

#[tokio::test]
async fn api_step_execution_is_bounded_by_max_concurrent_steps() {
    let engine =
        WorkflowEngine::with_limits(Duration::from_secs(5), None, Some(2)).expect("build engine");

    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_observed = Arc::new(AtomicUsize::new(0));

    {
        let in_flight = Arc::clone(&in_flight);
        let max_observed = Arc::clone(&max_observed);
        engine
            .register_async_function("track", move |_lua, _args: mlua::MultiValue| {
                let in_flight = Arc::clone(&in_flight);
                let max_observed = Arc::clone(&max_observed);
                async move {
                    let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_observed.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    Ok(mlua::Value::Nil)
                }
            })
            .expect("register track");
    }

    let script = r"
        local a = api.step(function() return track() end)
        local b = api.step(function() return track() end)
        local c = api.step(function() return track() end)
        local d = api.step(function() return track() end)
        api.join({ a, b, c, d })
        return true
    ";

    engine
        .run(script, serde_json::Value::Null)
        .await
        .expect("workflow run");

    let observed = max_observed.load(Ordering::SeqCst);
    assert!(
        observed <= 2,
        "expected at most 2 steps executing concurrently (max_concurrent_steps), observed {observed}"
    );
}

#[tokio::test]
async fn register_api_function_nests_the_callable_under_the_api_table() {
    let engine = WorkflowEngine::new().expect("build engine");
    engine
        .register_api_function("run", move |lua, args: mlua::MultiValue| {
            let id: String = mlua::FromLuaMulti::from_lua_multi(args, lua).unwrap_or_default();
            async move {
                Ok(mlua::Value::String(
                    lua.create_string(&format!("ran {id}"))?,
                ))
            }
        })
        .expect("register api.run");

    let script = r#"
        -- also proves api.step/api.join (installed by the engine itself)
        -- still work alongside a function registered via
        -- register_api_function, i.e. it's added to the same table, not
        -- replacing it.
        local step = api.step(function() return "stepped" end)
        return { ran = api.run("svc.op"), stepped = step:get() }
    "#;

    let result = engine
        .run(script, serde_json::Value::Null)
        .await
        .expect("workflow run");

    assert_eq!(
        result,
        serde_json::json!({ "ran": "ran svc.op", "stepped": "stepped" })
    );
}
