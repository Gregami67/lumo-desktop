//! IMAP and SMTP connections to the local Proton Mail Bridge.

use super::config::{validate_config, ConnectionSecurity, ProtonMailConfig};
use super::{ImapSession, BRIDGE_HOST, IO_TIMEOUT};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, Tokio1Executor};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::verify_server_name;
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::ParsedCertificate;
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;

pub(super) async fn connect_imap(
    config: &ProtonMailConfig,
    password: &str,
) -> Result<ImapSession, String> {
    validate_config(config)?;
    let tcp = timeout(
        IO_TIMEOUT,
        TcpStream::connect((BRIDGE_HOST, config.imap_port)),
    )
    .await
    .map_err(|_| "IMAP connection timed out; make sure Proton Mail Bridge is running".to_string())?
    .map_err(|e| describe_connection_error("IMAP", &e.to_string()))?;
    let server_name = ServerName::try_from(BRIDGE_HOST).map_err(|e| e.to_string())?;
    let tls_stream = match config.imap_security {
        ConnectionSecurity::Tls => timeout(
            IO_TIMEOUT,
            bridge_tls_connector()?.connect(server_name, tcp),
        )
        .await
        .map_err(|_| "IMAP TLS handshake timed out".to_string())?
        .map_err(|e| describe_connection_error("IMAP TLS", &e.to_string()))?,
        ConnectionSecurity::StartTls => {
            let mut client = async_imap::Client::new(tcp);
            timeout(IO_TIMEOUT, client.read_response())
                .await
                .map_err(|_| "IMAP greeting timed out".to_string())?
                .map_err(|e| describe_connection_error("IMAP", &e.to_string()))?
                .ok_or_else(|| {
                    "Bridge closed the IMAP connection before its greeting".to_string()
                })?;
            timeout(
                IO_TIMEOUT,
                client.run_command_and_check_ok("STARTTLS", None),
            )
            .await
            .map_err(|_| "IMAP STARTTLS timed out".to_string())?
            .map_err(|e| describe_connection_error("IMAP STARTTLS", &e.to_string()))?;
            timeout(
                IO_TIMEOUT,
                bridge_tls_connector()?.connect(server_name, client.into_inner()),
            )
            .await
            .map_err(|_| "IMAP TLS handshake timed out".to_string())?
            .map_err(|e| describe_connection_error("IMAP TLS", &e.to_string()))?
        }
    };
    let mut client = async_imap::Client::new(tls_stream);
    if config.imap_security == ConnectionSecurity::Tls {
        timeout(IO_TIMEOUT, client.read_response())
            .await
            .map_err(|_| "IMAP greeting timed out".to_string())?
            .map_err(|e| describe_connection_error("IMAP", &e.to_string()))?
            .ok_or_else(|| "Bridge closed the IMAP connection before its greeting".to_string())?;
    }
    timeout(IO_TIMEOUT, client.login(&config.username, password))
        .await
        .map_err(|_| "IMAP login timed out".to_string())?
        .map_err(|(e, _)| describe_connection_error("IMAP login", &e.to_string()))
}

/// Bridge serves a self-signed certificate that only macOS installs into the OS trust store, so the
/// chain is not verified, only that the certificate names `BRIDGE_HOST`. This is only safe because
/// the connection never leaves `BRIDGE_HOST`.
#[derive(Debug)]
pub(super) struct AcceptBridgeCertificate(pub(super) Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptBridgeCertificate {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        verify_server_name(&ParsedCertificate::try_from(end_entity)?, server_name)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

pub(super) fn bridge_tls_connector() -> Result<TlsConnector, String> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let tls_config = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("could not initialize TLS protocol versions: {e}"))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptBridgeCertificate(provider)))
        .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(tls_config)))
}

pub(super) fn smtp_transport(
    config: &ProtonMailConfig,
    password: &str,
) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
    validate_config(config)?;
    let tls_parameters = TlsParameters::builder(BRIDGE_HOST.into())
        .dangerous_accept_invalid_certs(true)
        .build()
        .map_err(|e| describe_connection_error("SMTP TLS", &e.to_string()))?;
    let tls = match config.smtp_security {
        ConnectionSecurity::StartTls => Tls::Required(tls_parameters),
        ConnectionSecurity::Tls => Tls::Wrapper(tls_parameters),
    };
    Ok(
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(BRIDGE_HOST)
            .port(config.smtp_port)
            .tls(tls)
            .credentials(Credentials::new(
                config.username.clone(),
                password.to_string(),
            ))
            .timeout(Some(IO_TIMEOUT))
            .build(),
    )
}

pub(super) fn security_name(security: ConnectionSecurity) -> &'static str {
    match security {
        ConnectionSecurity::StartTls => "start_tls",
        ConnectionSecurity::Tls => "tls",
    }
}

pub(super) fn describe_connection_error(service: &str, error: &str) -> String {
    if error.to_ascii_lowercase().contains("connection refused") {
        format!("{service} connection was refused. Make sure Proton Mail Bridge is running and the displayed port is correct.")
    } else {
        format!("{service} connection failed: {error}")
    }
}
