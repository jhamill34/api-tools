//! An [`InputPrompter`] adapter that pauses a running workflow and waits for
//! an external caller to supply the answer.

pub mod error;

use std::{
    collections::HashMap,
    sync::{
        mpsc::{self, Sender},
        Arc, Mutex,
    },
    time::Duration,
};

use execution_engine::services::InputPrompter;

/// Shared, thread-safe map from an in-flight execution's ID to the prompt
/// `params` it's waiting on and the channel its answer should be sent back
/// on. Populated by [`UserInput`], and drained from the other end whenever
/// an external caller answers a pending prompt.
pub type Signals = Arc<Mutex<HashMap<String, (serde_json::Value, Sender<serde_json::Value>)>>>;

/// An [`InputPrompter`] that blocks the calling execution on a channel until
/// an external caller answers via the shared [`Signals`] map, or the wait
/// times out.
pub struct UserInput {
    /// The map of pending prompts this instance registers into and reads
    /// answers from.
    signals: Signals,
}

impl UserInput {
    /// Creates a [`UserInput`] backed by the given, externally-shared
    /// `signals` map.
    #[must_use]
    #[inline]
    pub fn new(signals: Signals) -> Self {
        Self { signals }
    }

    /// Prompts and waits up to 60 seconds for an answer. See
    /// [`run_internal_with_timeout`](UserInput::run_internal_with_timeout).
    fn run_internal(
        &self,
        params: serde_json::Value,
        ctx: &execution_engine::services::EngineInputContext,
    ) -> error::Result<serde_json::Value> {
        self.run_internal_with_timeout(params, ctx, Duration::from_secs(60))
    }

    /// Registers `params` under `ctx.execution_id` in [`signals`](UserInput),
    /// blocks for up to `timeout` waiting for an answer, then removes the
    /// entry again — including when the wait times out, so a timed-out
    /// prompt never leaves a stale entry behind.
    fn run_internal_with_timeout(
        &self,
        params: serde_json::Value,
        ctx: &execution_engine::services::EngineInputContext,
        timeout: Duration,
    ) -> error::Result<serde_json::Value> {
        let rx = {
            let (tx, rx) = mpsc::channel::<serde_json::Value>();
            let mut signals = self
                .signals
                .lock()
                .map_err(|e| error::UserInput::PoisonedLock(e.to_string()))?;
            signals.insert(ctx.execution_id.clone(), (params, tx));
            rx
        };

        let result = rx.recv_timeout(timeout);

        {
            let mut signals = self
                .signals
                .lock()
                .map_err(|e| error::UserInput::PoisonedLock(e.to_string()))?;
            signals.remove(&ctx.execution_id);
        }

        Ok(result?)
    }
}

impl InputPrompter for UserInput {
    #[inline]
    fn run(
        &self,
        params: serde_json::Value,
        ctx: &execution_engine::services::EngineInputContext,
    ) -> execution_engine::error::Result<serde_json::Value> {
        let result = self.run_internal(params, ctx)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use execution_engine::services::EngineInputContext;

    use super::*;

    #[test]
    fn cleans_up_the_signal_entry_even_when_recv_times_out() {
        let signals: Signals = Arc::new(Mutex::new(HashMap::new()));
        let user_input = UserInput::new(Arc::clone(&signals));
        let ctx = EngineInputContext::new(None, "test-execution-id".to_owned(), false);

        let result = user_input.run_internal_with_timeout(
            serde_json::json!({}),
            &ctx,
            Duration::from_millis(10),
        );

        assert!(result.is_err(), "expected a timeout error, got {result:?}");
        assert!(
            signals.lock().unwrap().is_empty(),
            "expected the signal entry to be cleaned up after a timeout, not left behind"
        );
    }
}
