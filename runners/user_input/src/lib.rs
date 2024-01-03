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
        let rx = {
            let (tx, rx) = mpsc::channel::<serde_json::Value>();
            let mut signals = self
                .signals
                .lock()
                .map_err(|e| error::UserInput::PoisonedLock(e.to_string()))?;
            signals.insert(ctx.execution_id.clone(), (params, tx));
            rx
        };

        let value = rx.recv_timeout(Duration::from_secs(60))?;

        {
            let mut signals = self
                .signals
                .lock()
                .map_err(|e| error::UserInput::PoisonedLock(e.to_string()))?;
            signals.remove(&ctx.execution_id);
        };

        Ok(value)
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
