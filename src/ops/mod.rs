//! Operation handlers and runtime support.

pub mod backend;
pub mod env;
pub mod handlers;
pub(crate) mod reconcile;
pub mod agent;

use syncbat::{CoreBuilder, OperationRegisterItem};

/// Return the complete operation catalog.
#[must_use]
pub fn catalog() -> Vec<OperationRegisterItem> {
    let mut catalog = handlers::catalog();
    catalog.extend(agent::catalog());
    catalog
}

/// Register every built-in operation.
///
/// # Errors
///
/// Returns [`syncbat::BuildError`] if any descriptor or handler cannot be
/// registered with the builder.
pub fn register_all(builder: &mut CoreBuilder) -> Result<(), syncbat::BuildError> {
    handlers::register_all(builder)?;
    agent::register_all(builder)?;
    Ok(())
}
