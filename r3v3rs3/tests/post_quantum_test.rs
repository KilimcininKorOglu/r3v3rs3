//! The TLS key exchange of the default provider. The aws-lc-rs provider offers the post-quantum
//! X25519MLKEM768 hybrid, and a handshake must agree on it.

use r3v3rs3::certs::Cert;
use std::sync::Arc;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use tokio_rustls::rustls::{ClientConfig, NamedGroup, RootCertStore, ServerConfig};
use tokio_rustls::{TlsAcceptor, TlsConnector};

#[tokio::test]
async fn a_tls_handshake_negotiates_the_post_quantum_group() -> anyhow::Result<()> {
    let root = Arc::new(Cert::new_ca().unwrap());
    let cert = Arc::new(Cert::new_self_signed(&["localhost".parse().unwrap()], &root).unwrap());

    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            CertificateDer::pem_slice_iter(&cert.pem_chain).collect::<Result<Vec<_>, _>>()?,
            PrivateKeyDer::from_pem_slice(cert.pem_key.as_ref().unwrap())?,
        )?;
    let mut roots = RootCertStore::empty();
    for der in CertificateDer::pem_slice_iter(&root.pem_chain) {
        roots.add(der?)?;
    }
    let client_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    let (client, server) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        TlsAcceptor::from(Arc::new(server_config))
            .accept(server)
            .await
    });
    let name = ServerName::try_from("localhost")?;
    let client = tokio::spawn(async move {
        TlsConnector::from(Arc::new(client_config))
            .connect(name, client)
            .await
    });

    let (server, client) = tokio::join!(server, client);
    let server = server??;
    let client = client??;

    let group = |conn: &tokio_rustls::rustls::CommonState| {
        conn.negotiated_key_exchange_group()
            .map(|group| group.name())
    };
    let server_group = group(server.get_ref().1);
    let client_group = group(client.get_ref().1);
    assert_eq!(server_group, client_group);
    assert_eq!(server_group, Some(NamedGroup::X25519MLKEM768));
    Ok(())
}
