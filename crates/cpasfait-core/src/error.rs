use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("remote backend error: {0}")]
    Backend(String),
    #[error("encryption: {0}")]
    Crypto(String),
    /// This device has no key for a shared tab (the payload is its public key).
    #[error("no key for this tab yet: ask a member to run `:tabinvite {0}`")]
    NotInvited(String),
}

pub type Result<T> = std::result::Result<T, Error>;
