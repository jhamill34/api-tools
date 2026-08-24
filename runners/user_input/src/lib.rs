#![warn(clippy::restriction, clippy::pedantic)]
#![allow(
    clippy::blanket_clippy_restriction_lints,
    clippy::mod_module_files,
    clippy::self_named_module_files,

    clippy::implicit_return,
    clippy::shadow_reuse,
    clippy::match_ref_pats,

    // Would like to turn on (Configured to 50?)
    clippy::too_many_lines,
    clippy::question_mark_used,
    clippy::needless_borrowed_reference,
    clippy::absolute_paths,
    clippy::ref_patterns,
    clippy::single_call_fn,
    clippy::min_ident_chars,
)]

//!

pub mod error;

extern crate alloc;
use alloc::sync::Arc;
use core::time::Duration;

use std::{
    collections::HashMap,
    sync::{
        mpsc::{self, Sender},
        Mutex,
    },
};

use execution_engine::services::InputPrompter;

///
pub type Signals = Arc<Mutex<HashMap<String, (serde_json::Value, Sender<serde_json::Value>)>>>;

///
pub struct UserInput {
    ///
    signals: Signals,
}

impl UserInput {
    ///
    #[must_use]
    #[inline]
    pub fn new(signals: Signals) -> Self {
        Self { signals }
    }

    ///
    fn run_internal(
        &self,
        params: serde_json::Value,
        ctx: &execution_engine::services::EngineInputContext,
    ) -> error::Result<serde_json::Value> {
        self.run_internal_with_timeout(params, ctx, Duration::from_secs(60))
    }

    ///
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
