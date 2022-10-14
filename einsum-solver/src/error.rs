use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Invlaid subscripts: {0}")]
    InvalidSubScripts(String),

    #[error("Unknown index {0}")]
    UnknownIndex(char),
}