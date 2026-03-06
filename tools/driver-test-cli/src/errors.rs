use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("vm error: {0}")]
    Vm(String),
    #[error("detection error: {0}")]
    Detection(String),
    #[error("{0}")]
    Deploy(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl From<crate::deploy::DeployError> for AppError {
    fn from(d: crate::deploy::DeployError) -> Self {
        AppError::Deploy(d.to_string())
    }
}
