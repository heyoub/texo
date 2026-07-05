/// Crate-wide texo error.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum TexoError {
    /// Filesystem I/O error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// `BatPak` store error.
    #[error("store: {0}")]
    Store(#[from] batpak::store::StoreError),
    /// Journal decode error for an entity stream.
    #[error("journal decode for {entity}: {detail}")]
    Decode {
        /// Entity stream that could not be decoded.
        entity: String,
        /// Decode failure detail.
        detail: String,
    },
    /// Domain identifier parse error.
    #[error("domain id: {0}")]
    IdParse(#[from] crate::events::ids::IdParseError),
}

impl TexoError {
    /// Stable machine-readable error code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "io",
            Self::Store(_) => "store",
            Self::Decode { .. } => "journal.decode",
            Self::IdParse(_) => "domain.id",
        }
    }
}
