use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
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
        api.terminal({ a, b })
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
        -- api.terminal accepts a single handle directly (not just a table)
        -- and shares the same memoized cache as :get() - proves a third,
        -- differently-shaped access still doesn't re-run the underlying call.
        local third = api.terminal(step)
        return { first = first, second = second, third = third }
    "#;

    let result = engine
        .run(script, serde_json::Value::Null)
        .await
        .expect("workflow run");

    assert_eq!(result["first"], serde_json::json!("A"));
    assert_eq!(result["second"], serde_json::json!("A"));
    assert_eq!(result["third"], serde_json::json!("A"));
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "expected the underlying db_lookup call to run exactly once, not once per resolution"
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
        local step_a = api.step(function() return db_lookup("A") end)
        local a = step_a:get()
        local step_b = api.step(function() return db_lookup("B") end)
        local b = step_b:get()
        api.terminal({ step_a, step_b })
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
async fn a_step_does_not_start_until_its_declared_dependency_resolves() {
    let engine = WorkflowEngine::new().expect("build engine");

    let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

    {
        let order = Arc::clone(&order);
        engine
            .register_async_function("mark_a", move |_lua, _args: mlua::MultiValue| {
                let order = Arc::clone(&order);
                async move {
                    // Deliberately the slower of the two - if `b` were
                    // (incorrectly) not gated on `a`, it would trivially
                    // finish and log first since it has no delay at all.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    order.lock().unwrap().push("a");
                    Ok(mlua::Value::Nil)
                }
            })
            .expect("register mark_a");
    }
    {
        let order = Arc::clone(&order);
        engine
            .register_async_function("mark_b", move |_lua, _args: mlua::MultiValue| {
                let order = Arc::clone(&order);
                async move {
                    order.lock().unwrap().push("b");
                    Ok(mlua::Value::Nil)
                }
            })
            .expect("register mark_b");
    }

    let script = r"
        local a = api.step(function() return mark_a() end)
        local b = api.step(function() return mark_b() end, { a })
        return api.terminal(b)
    ";

    engine
        .run(script, serde_json::Value::Null)
        .await
        .expect("workflow run");

    assert_eq!(
        order.lock().unwrap().as_slice(),
        ["a", "b"],
        "expected b to run only after its dependency a resolved"
    );
}

#[tokio::test]
async fn a_step_waits_for_all_of_multiple_dependencies_concurrently() {
    let engine = WorkflowEngine::new().expect("build engine");
    install_mock_db_lookup(
        &engine,
        Duration::from_millis(100),
        Arc::new(AtomicUsize::new(0)),
    );

    let script = r#"
        local a = api.step(function() return db_lookup("A") end)
        local b = api.step(function() return db_lookup("B") end)
        local c = api.step(function() return "C" end, { a, b })
        return api.terminal(c)
    "#;

    let start = Instant::now();
    let result = engine
        .run(script, serde_json::Value::Null)
        .await
        .expect("workflow run");
    let elapsed = start.elapsed();

    assert_eq!(result, serde_json::json!("C"));
    assert!(
        elapsed < Duration::from_millis(180),
        "expected c's two dependencies (a and b, both ~100ms) to resolve concurrently before c \
         ran, took {elapsed:?} - if this is slow (~200ms+), deps are being resolved sequentially"
    );
}

#[tokio::test]
async fn a_failed_dependency_short_circuits_the_dependent_step_without_running_it() {
    let engine = WorkflowEngine::new().expect("build engine");

    let b_ran = Arc::new(AtomicUsize::new(0));
    {
        let b_ran = Arc::clone(&b_ran);
        engine
            .register_async_function("mark_b_ran", move |_lua, _args: mlua::MultiValue| {
                let b_ran = Arc::clone(&b_ran);
                async move {
                    b_ran.fetch_add(1, Ordering::SeqCst);
                    Ok(mlua::Value::Nil)
                }
            })
            .expect("register mark_b_ran");
    }

    let script = r#"
        local a = api.step(function() error("boom") end)
        local b = api.step(function() return mark_b_ran() end, { a })
        return api.terminal(b)
    "#;

    let result = engine.run(script, serde_json::Value::Null).await;

    assert!(
        result.is_err(),
        "expected the run to fail via the failed dependency"
    );
    assert_eq!(
        b_ran.load(Ordering::SeqCst),
        0,
        "expected b's body to never run since its dependency a failed"
    );
}

#[tokio::test]
async fn a_step_not_reachable_from_the_terminal_never_runs() {
    let engine = WorkflowEngine::new().expect("build engine");

    let unused_ran = Arc::new(AtomicUsize::new(0));
    {
        let unused_ran = Arc::clone(&unused_ran);
        engine
            .register_async_function("mark_unused_ran", move |_lua, _args: mlua::MultiValue| {
                let unused_ran = Arc::clone(&unused_ran);
                async move {
                    unused_ran.fetch_add(1, Ordering::SeqCst);
                    Ok(mlua::Value::Nil)
                }
            })
            .expect("register mark_unused_ran");
    }

    let script = r#"
        local used = api.step(function() return "used" end)
        -- Registered, but never :get(), never joined, never a dependency,
        -- and not passed to api.terminal - unreachable, so it must never
        -- run at all (not "start then get abandoned mid-flight", which is
        -- what the engine's original eager design did - see issue #106).
        local unused = api.step(function() return mark_unused_ran() end)
        return api.terminal(used)
    "#;

    let result = engine
        .run(script, serde_json::Value::Null)
        .await
        .expect("workflow run");

    assert_eq!(result, serde_json::json!("used"));
    assert_eq!(
        unused_ran.load(Ordering::SeqCst),
        0,
        "expected the unreachable step's body to never run"
    );
}

#[tokio::test]
async fn run_errors_when_api_step_is_used_without_calling_api_terminal() {
    let engine = WorkflowEngine::new().expect("build engine");

    let script = r#"
        local a = api.step(function() return "A" end)
        return a:get()
    "#;

    let err = engine
        .run(script, serde_json::Value::Null)
        .await
        .expect_err("expected the run to fail without a terminal declared");

    let message = err.to_string();
    assert!(
        message.contains("api.terminal"),
        "expected an error mentioning api.terminal, got: {message}"
    );
}

#[tokio::test]
async fn a_runaway_script_is_aborted_after_its_time_budget() {
    let engine = WorkflowEngine::with_limits(Duration::from_millis(50), None, None, None)
        .expect("build engine");

    let result = engine
        .run("while true do end", serde_json::Value::Null)
        .await;

    assert!(result.is_err(), "expected the runaway script to be aborted");
}

#[tokio::test]
async fn a_script_that_exceeds_its_memory_budget_is_aborted() {
    let engine = WorkflowEngine::with_limits(Duration::from_secs(5), Some(1024 * 1024), None, None)
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
    let engine = WorkflowEngine::with_limits(Duration::from_secs(5), None, Some(2), None)
        .expect("build engine");

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
        api.terminal({ a, b, c, d })
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
        -- also proves api.step/api.terminal (installed by the engine
        -- itself) still work alongside a function registered via
        -- register_api_function, i.e. it's added to the same table, not
        -- replacing it.
        local step = api.step(function() return "stepped" end)
        local ran = api.run("svc.op")
        local stepped = api.terminal(step)
        return { ran = ran, stepped = stepped }
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

#[tokio::test]
async fn api_step_registration_up_to_max_steps_succeeds() {
    let engine = WorkflowEngine::with_limits(Duration::from_secs(5), None, None, Some(3))
        .expect("build engine");

    let script = r"
        local handles = {}
        for i = 1, 3 do
            handles[i] = api.step(function() return i end)
        end
        return api.terminal(handles)
    ";

    let result = engine
        .run(script, serde_json::Value::Null)
        .await
        .expect("registering exactly max_steps steps should succeed");

    assert_eq!(result, serde_json::json!([1, 2, 3]));
}

#[tokio::test]
async fn api_step_registration_is_rejected_once_max_steps_is_exceeded() {
    let engine = WorkflowEngine::with_limits(Duration::from_secs(5), None, None, Some(3))
        .expect("build engine");

    let script = r"
        for i = 1, 4 do
            api.step(function() return i end)
        end
        return true
    ";

    let err = engine
        .run(script, serde_json::Value::Null)
        .await
        .expect_err("expected the 4th api.step registration to be rejected");

    let message = err.to_string();
    assert!(
        message.contains("too many steps"),
        "expected a 'too many steps' error, got: {message}"
    );
}

#[tokio::test]
async fn many_steps_resolve_correctly_via_terminal() {
    // General stress/correctness coverage for the redesigned resolution
    // path (issue #106: recursive resolve() + join_all + memoization,
    // replacing the earlier PendingQueue-draining driver loop issue #105
    // exercised) at a scale well beyond what the other tests exercise - a
    // bug here would show up as a missing/duplicated/wrong result.
    let engine = WorkflowEngine::new().expect("build engine");
    let call_count = Arc::new(AtomicUsize::new(0));
    install_mock_db_lookup(&engine, Duration::from_millis(1), Arc::clone(&call_count));

    let script = r"
        local handles = {}
        for i = 1, 200 do
            handles[i] = api.step(function() return db_lookup(tostring(i)) end)
        end
        return api.terminal(handles)
    ";

    let result = engine
        .run(script, serde_json::Value::Null)
        .await
        .expect("workflow run");

    let expected: Vec<serde_json::Value> = (1..=200)
        .map(|i| serde_json::json!(i.to_string()))
        .collect();
    assert_eq!(result, serde_json::Value::Array(expected));
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        200,
        "expected every one of the 200 steps to run exactly once"
    );
}
