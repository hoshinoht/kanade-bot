#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Configuration(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    Startup(String),
}

impl Error {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) => 64,
            Self::Unavailable(_) => 69,
            Self::Configuration(_) | Self::Startup(_) => 78,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Configuration(_) => "configuration",
            Self::Unavailable(_) => "unavailable",
            Self::Usage(_) => "usage",
            Self::Startup(_) => "startup",
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
}
