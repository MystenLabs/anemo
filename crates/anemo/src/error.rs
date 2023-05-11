pub use anyhow::{Error, Result};

/// Alias for a type-erased error type.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

// As recommended in https://github.com/dtolnay/anyhow/issues/63#issuecomment-591011454.
#[derive(thiserror::Error, Debug)]
#[error(transparent)]
pub struct AsStdError(#[from] anyhow::Error);
