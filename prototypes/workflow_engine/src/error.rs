/// Errors from running a workflow script. `Clone` so a memoized step result
/// (`Result<Value, WorkflowError>`) can be handed out to more than one
/// `:get()` caller.
#[derive(Debug, Clone, thiserror::Error)]
pub enum WorkflowError {
    #[error("lua error: {0}")]
    Lua(String),
    #[error("workflow exceeded its execution time budget")]
    Timeout,
    #[error("workflow exceeded its memory budget")]
    MemoryLimitExceeded,
    #[error("value conversion error: {0}")]
    Conversion(String),
    #[error("workflow step dependency failed: {0}")]
    DependencyFailed(String),
    #[error(
        "workflow used api.step ({0} step(s) registered) but never called api.terminal to \
         declare its output"
    )]
    MissingTerminal(usize),
}

impl From<mlua::Error> for WorkflowError {
    fn from(err: mlua::Error) -> Self {
        Self::Lua(err.to_string())
    }
}

impl From<WorkflowError> for mlua::Error {
    fn from(err: WorkflowError) -> Self {
        mlua::Error::ExternalError(std::sync::Arc::new(err))
    }
}

pub type Result<T> = std::result::Result<T, WorkflowError>;
