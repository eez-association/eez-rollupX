//! Transport policy for the v2 control service. Local development uses literal
//! loopback addresses. Remote peers must present operator-provisioned identities.

use std::{
    collections::HashSet,
    fs,
    net::{IpAddr, SocketAddr},
    path::Path,
    sync::Arc,
    time::Duration,
};

use alloy_primitives::B256;
use eyre::{Context, Result, bail, ensure, eyre};
use rustls::pki_types::{CertificateDer, pem::PemObject};
use sha2::{Digest, Sha256};
use tonic::{
    Request, Status,
    transport::{Certificate, ClientTlsConfig, Endpoint, Identity, Server, ServerTlsConfig},
};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

fn pem(directory: &Path, name: &str) -> Result<Vec<u8>> {
    let path = directory.join(name);
    fs::read(&path).wrap_err_with(|| format!("read {}", path.display()))
}

fn credentials(directory: &Path) -> Result<(Certificate, Identity)> {
    ensure!(
        directory.is_absolute(),
        "prover TLS directory must be absolute"
    );
    let ca = pem(directory, "ca.pem")?;
    // Tonic silently skips unusable roots. A malformed local member config
    // must fail startup even when other members could satisfy the threshold.
    let mut roots = rustls::RootCertStore::empty();
    for certificate in CertificateDer::pem_slice_iter(&ca) {
        roots
            .add(certificate.wrap_err("decode prover CA PEM")?)
            .wrap_err("invalid prover trust anchor")?;
    }
    ensure!(!roots.is_empty(), "ca.pem contains no trust anchors");
    Ok((
        Certificate::from_pem(ca),
        Identity::from_pem(pem(directory, "cert.pem")?, pem(directory, "key.pem")?),
    ))
}

/// Construct and validate a member endpoint before quorum admission. HTTPS
/// credentials live under the exact configured authority within the client root.
pub fn client_endpoint(url: &str, root: Option<&Path>) -> Result<Endpoint> {
    let endpoint =
        Endpoint::from_shared(url.to_owned()).map_err(|_| eyre!("invalid prover URL"))?;
    let uri = endpoint.uri();
    let authority = uri
        .authority()
        .ok_or_else(|| eyre!("missing prover authority"))?;
    ensure!(
        !authority.as_str().contains('@')
            && !url.contains('#')
            && uri.query().is_none()
            && matches!(uri.path(), "" | "/"),
        "prover URL must be an origin without credentials, path, query or fragment"
    );
    let host = authority.host();
    ensure!(
        authority.as_str() == host || authority.port_u16().is_some_and(|port| port != 0),
        "invalid prover port"
    );
    let host = host
        .strip_prefix('[')
        .and_then(|ip| ip.strip_suffix(']'))
        .unwrap_or(host);
    match uri.scheme_str() {
        Some("http") => {
            ensure!(
                root.is_none(),
                "configured prover TLS requires HTTPS for every member"
            );
            ensure!(
                host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()),
                "plaintext prover URL requires a literal loopback address"
            );
            Ok(endpoint.connect_timeout(HANDSHAKE_TIMEOUT))
        }
        Some("https") => {
            let root = root.ok_or_else(|| eyre!("HTTPS prover requires EEZ_PROVER_TLS_DIR"))?;
            ensure!(root.is_absolute(), "EEZ_PROVER_TLS_DIR must be absolute");
            ensure!(
                host != "." && host != ".." && !host.is_empty(),
                "invalid prover hostname"
            );
            let (ca, identity) = credentials(&root.join(authority.as_str()))?;
            let tls = ClientTlsConfig::new()
                .domain_name(host)
                .ca_certificate(ca)
                .identity(identity)
                .timeout(HANDSHAKE_TIMEOUT);
            Ok(endpoint.tls_config(tls)?.connect_timeout(HANDSHAKE_TIMEOUT))
        }
        _ => bail!("prover URL requires http or https"),
    }
}

/// A local attachment, or a remote Composer's authenticated leaf fingerprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    Loopback,
    Certificate(B256),
}

/// The same immutable grants are used for discovery and every new attachment.
#[derive(Clone, Debug)]
pub enum CallerPolicy {
    Loopback,
    Certificates(Arc<HashSet<B256>>),
}

impl CallerPolicy {
    pub fn authenticate<T>(&self, request: &Request<T>) -> Result<Caller, Status> {
        let Self::Certificates(allowed) = self else {
            return Ok(Caller::Loopback);
        };
        let certificates = request
            .peer_certs()
            .ok_or_else(|| Status::unauthenticated("missing client certificate"))?;
        let leaf = certificates
            .first()
            .ok_or_else(|| Status::unauthenticated("empty client certificate chain"))?;
        let fingerprint = B256::from(<[u8; 32]>::from(Sha256::digest(leaf.as_ref())));
        if !allowed.contains(&fingerprint) {
            return Err(Status::permission_denied(
                "client certificate is not authorized",
            ));
        }
        Ok(Caller::Certificate(fingerprint))
    }
}

/// Build the listener and the matching RPC admission policy as a pair. The TLS
/// verifier checks the peer chain; the RPC policy additionally grants exact leaves.
pub fn server_transport(
    addr: SocketAddr,
    directory: Option<&Path>,
) -> Result<(Server, CallerPolicy)> {
    let Some(directory) = directory else {
        ensure!(
            addr.ip().is_loopback(),
            "non-loopback prover listener requires EEZ_PROOF_SIGNER_TLS_DIR"
        );
        return Ok((Server::builder(), CallerPolicy::Loopback));
    };
    let (ca, identity) = credentials(directory)?;
    let grants = fs::read_to_string(directory.join("allowed-clients.sha256"))
        .wrap_err("read allowed-clients.sha256")?;
    let mut allowed = HashSet::new();
    for line in grants.lines() {
        let fingerprint: B256 = line
            .trim()
            .parse()
            .wrap_err("invalid client certificate SHA-256 fingerprint")?;
        ensure!(
            allowed.insert(fingerprint),
            "duplicate client certificate grant"
        );
    }
    ensure!(!allowed.is_empty(), "empty client certificate grants");
    // Tonic's server uses rustls's process provider. The shipping workspace
    // links both ring and aws-lc through different dependencies.
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let tls = ServerTlsConfig::new()
        .identity(identity)
        .client_ca_root(ca)
        .timeout(HANDSHAKE_TIMEOUT);
    Ok((
        Server::builder().tls_config(tls)?,
        CallerPolicy::Certificates(Arc::new(allowed)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_requires_literal_loopback_and_absent_credentials() {
        for url in [
            "http://127.0.0.1:50061",
            "http://[::1]:50061",
            "http://127.0.1.2",
        ] {
            assert!(client_endpoint(url, None).is_ok(), "{url}");
            assert!(
                client_endpoint(url, Some(Path::new("/tls"))).is_err(),
                "{url}"
            );
        }
        for url in [
            "http://localhost",
            "http://0.0.0.0",
            "http://192.0.2.1",
            "http://[::]",
            "http://127.0.0.1:bad",
            "http://127.0.0.1:99999",
            "http://127.0.0.1/path",
            "http://127.0.0.1?k=v",
            "http://user@127.0.0.1",
            "http://127.0.0.1/#fragment",
            "https://127.0.0.1:50061",
            "unix:/tmp/socket",
        ] {
            assert!(client_endpoint(url, None).is_err(), "{url}");
        }
        for address in ["0.0.0.0:50061", "[::]:50061", "192.0.2.1:50061"] {
            assert!(
                server_transport(address.parse().unwrap(), None).is_err(),
                "{address}"
            );
        }
    }
}
