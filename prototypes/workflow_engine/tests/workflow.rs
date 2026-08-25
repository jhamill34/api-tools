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
    let result = engine.run(script).await.expect("workflow run");
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

    let result = engine.run(script).await.expect("workflow run");

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
    engine.run(script).await.expect("workflow run");
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
        WorkflowEngine::with_limits(Duration::from_millis(50), None).expect("build engine");

    let result = engine.run("while true do end").await;

    assert!(result.is_err(), "expected the runaway script to be aborted");
}

#[tokio::test]
async fn a_script_that_exceeds_its_memory_budget_is_aborted() {
    let engine = WorkflowEngine::with_limits(Duration::from_secs(5), Some(1024 * 1024))
        .expect("build engine");

    let script = r#"
        local t = {}
        for i = 1, 10000000 do
            t[i] = string.rep("x", 1000)
        end
        return "should not get here"
    "#;

    let result = engine.run(script).await;

    assert!(
        result.is_err(),
        "expected the script exceeding its memory budget to be aborted"
    );
}
