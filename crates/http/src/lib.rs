//! The one place termide's HTTP clients are configured.
//!
//! Every `ureq` agent starts from [`agent_builder`], so they all verify TLS
//! against the same roots: the bundled Mozilla set (webpki-roots) *and* the
//! system's trust store (rustls-native-certs, which also reads
//! `SSL_CERT_FILE` and `SSL_CERT_DIR`). A corporate CA or a self-signed
//! certificate the user has trusted on the machine is thus accepted, while a
//! machine whose store is missing or unreadable still reaches public hosts
//! through the bundled set. There is no switch to disable verification.

use std::sync::{Arc, OnceLock};

/// A `ureq` agent builder with termide's TLS setup; add timeouts, a user
/// agent and the rest, then `build()`.
pub fn agent_builder() -> ureq::AgentBuilder {
    ureq::AgentBuilder::new().tls_config(tls_config())
}

/// The shared client config, built once: loading the system store costs
/// tens of milliseconds on some platforms.
fn tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            // An explicit provider, as ureq does: `builder()` panics when the
            // process links more than one rustls provider and set no default.
            let config = rustls::ClientConfig::builder_with_provider(
                rustls::crypto::ring::default_provider().into(),
            )
            .with_protocol_versions(&[&rustls::version::TLS12, &rustls::version::TLS13])
            .expect("the ring provider supports TLS 1.2 and 1.3")
            .with_root_certificates(root_store())
            .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

fn root_store() -> rustls::RootCertStore {
    let mut roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let native = rustls_native_certs::load_native_certs();
    for error in &native.errors {
        log::warn!("tls: system certificates: {error}");
    }
    let (added, ignored) = roots.add_parsable_certificates(native.certs);
    log::debug!("tls: {added} system root certificates added, {ignored} unparsable ignored");
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_hold_at_least_the_bundled_set() {
        assert!(root_store().len() >= webpki_roots::TLS_SERVER_ROOTS.len());
    }

    #[test]
    fn config_is_built_once() {
        assert!(Arc::ptr_eq(&tls_config(), &tls_config()));
    }
}
