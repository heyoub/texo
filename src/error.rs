/// Crate-wide texo error.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum TexoError {
    /// Filesystem I/O error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl TexoError {
    /// Stable machine-readable error code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "io",
        }
    }
}
