//! TLS bootstrap for the crate's HTTPS clients.
//!
//! `reqwest` is built with the `rustls-no-provider` feature so the crate pulls
//! in the rustls backend (and `rustls-platform-verifier` for CA roots) without
//! dragging in a crypto provider that needs a C toolchain — `aws-lc-rs` requires
//! cmake + clang and breaks slim/musl Docker builds. Because no provider is
//! compiled in by default, rustls 0.23 has no process-default
//! [`CryptoProvider`](rustls::crypto::CryptoProvider) installed, and every
//! HTTPS request would otherwise fail. [`ensure_crypto_provider`] installs the
//! pure-Rust `ring` provider; the crate's reqwest client constructors call it so
//! consumers don't have to.

/// Install the process-default rustls crypto provider (ring) for the HTTPS
/// clients. Idempotent and safe to call repeatedly; a no-op if a provider is
/// already installed (e.g. by the host binary).
pub fn ensure_crypto_provider() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}
