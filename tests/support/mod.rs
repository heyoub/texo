//! Shared integration-test helpers.

use serde_json::{json, Value};
use tempfile::TempDir;
use texo::host::TexoHost;

/// Pinned observation timestamp for deterministic fixtures.
pub const OBSERVED_AT_MS: u64 = 1_700_000_000_000;

/// Integration test result type.
pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Test workspace with a runnable host.
pub struct TestWorkspace {
    /// Temporary root.
    pub dir: TempDir,
    /// Host.
    pub host: TexoHost,
}

impl TestWorkspace {
    /// Create a new initialized demo workspace.
    ///
    /// # Errors
    ///
    /// Returns an error when the tempdir, host, or init op fails.
    pub fn new() -> TestResult<Self> {
        let dir = TempDir::new()?;
        let mut host = TexoHost::open(dir.path(), "demo", OBSERVED_AT_MS)?;
        let _output = host.invoke_json("texo.workspace.init", &json!({"workspace_id": "demo"}))?;
        Ok(Self { dir, host })
    }

    /// Write a file under the temp root.
    ///
    /// # Errors
    ///
    /// Returns an error when parent creation or file writing fails.
    pub fn write(&self, path: &str, text: &str) -> TestResult {
        let full = self.dir.path().join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(full, text)?;
        Ok(())
    }

    /// Invoke a host operation.
    ///
    /// # Errors
    ///
    /// Returns an error when the operation fails.
    pub fn invoke(&mut self, op: &str, input: &Value) -> Result<Value, texo::error::TexoError> {
        self.host.invoke_json(op, input)
    }
}
