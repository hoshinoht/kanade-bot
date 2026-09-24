use std::sync::Arc;

use rustls::crypto::CryptoProvider;

/// Install the application's chosen TLS crypto provider.
///
/// Ring is the recorded spike choice. The unified dependency graph enables
/// more than one rustls backend, so auto-detection from crate features is
/// ambiguous and any `ClientConfig::builder()` / `ServerConfig::builder()`
/// use would panic. Call this once at process startup, before any builder
/// use; returns true when this call installed the provider.
pub fn install_default_provider() -> bool {
    rustls::crypto::ring::default_provider()
        .install_default()
        .is_ok()
}

/// Process-default provider proof hook for the production startup seam.
pub fn default_provider() -> Option<&'static Arc<CryptoProvider>> {
    CryptoProvider::get_default()
}
