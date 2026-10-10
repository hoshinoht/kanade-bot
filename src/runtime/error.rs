use std::fmt;

#[derive(Debug)]
pub enum Error {
    Configuration(String),
    Unavailable(String),
    Usage(String),
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

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(message)
            | Self::Unavailable(message)
            | Self::Usage(message)
            | Self::Startup(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for Error {}
