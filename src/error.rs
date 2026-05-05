use thiserror::Error;

#[derive(Debug, Error)]
pub enum UndercoverError {
    #[error("{0}")]
    Message(String),
}
