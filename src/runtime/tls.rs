use super::error::Error;

/// Install ring before any Rustls client or server builder is constructed.
pub fn install_ring_provider() -> Result<(), Error> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| {
            Error::Startup("a Rustls crypto provider was installed before bootstrap".into())
        })
}
