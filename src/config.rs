use crate::{crypto::CertVerifier, Result};
use pkcs8::EncodePrivateKey;
// use ed25519::pkcs8::EncodePrivateKey;
use quinn::IdleTimeout;
use rcgen::{CertificateParams, KeyPair, SignatureAlgorithm};
use std::{sync::Arc, time::Duration};

#[derive(Debug, Default)]
pub struct EndpointConfigBuilder {
    pub keypair: Option<ed25519_dalek::Keypair>,

    // TODO Maybe use server name to identify the network name?
    //
    /// Note that the end-entity certificate must have the
    /// [Subject Alternative Name](https://tools.ietf.org/html/rfc6125#section-4.1)
    /// extension to describe, e.g., the valid DNS name.
    pub server_name: Option<String>,

    /// How long to wait to hear from a peer before timing out a connection.
    ///
    /// In the absence of any keep-alive messages, connections will be closed if they remain idle
    /// for at least this duration.
    ///
    /// If unspecified, this will default to [`EndpointConfig::DEFAULT_IDLE_TIMEOUT`].
    ///
    /// Maximum possible value is 2^62.
    pub idle_timeout: Option<Duration>,

    /// Interval at which to send keep-alives to maintain otherwise idle connections.
    ///
    /// Keep-alives prevent otherwise idle connections from timing out.
    ///
    /// If unspecified, this will default to `None`, disabling keep-alives.
    pub keep_alive_interval: Option<Duration>,
}

impl EndpointConfigBuilder {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn build(self) -> Result<EndpointConfig> {
        let keypair = self.keypair.unwrap();
        let server_name = self.server_name.unwrap();
        let idle_timeout = self
            .idle_timeout
            .unwrap_or(EndpointConfig::DEFAULT_IDLE_TIMEOUT);
        let keep_alive_interval = self.keep_alive_interval;

        let cert_verifier = Arc::new(CertVerifier(server_name.clone()));
        let (certificate, pkcs8_der) = Self::generate_cert(&keypair, &server_name);

        let transport_config = Self::transport_config(idle_timeout, keep_alive_interval)?;
        let server_config = Self::server_config(
            certificate.clone(),
            pkcs8_der.clone(),
            cert_verifier.clone(),
            transport_config.clone(),
        )?;
        let client_config = Self::client_config(
            certificate.clone(),
            pkcs8_der.clone(),
            cert_verifier,
            transport_config,
        )?;

        Ok(EndpointConfig {
            _certificate: certificate,
            _pkcs8_der: pkcs8_der,
            keypair,
            quinn_server_config: server_config,
            quinn_client_config: client_config,
            server_name,
        })
    }

    fn generate_cert(
        keypair: &ed25519_dalek::Keypair,
        server_name: &str,
    ) -> (rustls::Certificate, rustls::PrivateKey) {
        let key_der = rustls::PrivateKey(keypair.to_pkcs8_bytes());
        let keypair = ed25519_dalek::Keypair::from_bytes(&keypair.to_bytes()).unwrap();
        let certificate = keypair_to_certificate(vec![server_name.to_owned()], keypair).unwrap();
        (certificate, key_der)
    }

    fn transport_config(
        idle_timeout: Duration,
        keep_alive_interval: Option<Duration>,
    ) -> Result<Arc<quinn::TransportConfig>> {
        let idle_timeout = IdleTimeout::try_from(idle_timeout)?;
        let mut config = quinn::TransportConfig::default();

        config
            .max_idle_timeout(Some(idle_timeout))
            .keep_alive_interval(keep_alive_interval);

        Ok(Arc::new(config))
    }

    fn server_config(
        cert: rustls::Certificate,
        pkcs8_der: rustls::PrivateKey,
        cert_verifier: Arc<CertVerifier>,
        transport_config: Arc<quinn::TransportConfig>,
    ) -> Result<quinn::ServerConfig> {
        let server_crypto = rustls::ServerConfig::builder()
            .with_safe_defaults()
            .with_client_cert_verifier(cert_verifier)
            .with_single_cert(vec![cert], pkcs8_der)?;

        let mut server = quinn::ServerConfig::with_crypto(Arc::new(server_crypto));
        server.transport = transport_config;
        Ok(server)
    }

    fn client_config(
        cert: rustls::Certificate,
        pkcs8_der: rustls::PrivateKey,
        cert_verifier: Arc<CertVerifier>,
        transport_config: Arc<quinn::TransportConfig>,
    ) -> Result<quinn::ClientConfig> {
        // setup certificates
        let mut roots = rustls::RootCertStore::empty();
        roots.add(&cert).map_err(|_e| ConfigError::Webpki)?;

        let client_crypto = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_custom_certificate_verifier(cert_verifier)
            .with_single_cert(vec![cert], pkcs8_der)?;

        let mut client = quinn::ClientConfig::new(Arc::new(client_crypto));
        client.transport = transport_config;
        Ok(client)
    }
}

#[derive(Debug)]
pub struct EndpointConfig {
    _certificate: rustls::Certificate,
    _pkcs8_der: rustls::PrivateKey,
    keypair: ed25519_dalek::Keypair,
    quinn_server_config: quinn::ServerConfig,
    quinn_client_config: quinn::ClientConfig,

    // TODO Maybe use server name to identify the network name?
    //
    /// Note that the end-entity certificate must have the
    /// [Subject Alternative Name](https://tools.ietf.org/html/rfc6125#section-4.1)
    /// extension to describe, e.g., the valid DNS name.
    server_name: String,
}

impl EndpointConfig {
    /// Default for [`EndpointConfig::idle_timeout`] (30 seconds).
    ///
    /// This is based on average time in which routers would close the UDP mapping to the peer if they
    /// see no conversation between them.
    pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

    pub fn builder() -> EndpointConfigBuilder {
        EndpointConfigBuilder::new()
    }

    pub fn keypair(&self) -> &ed25519_dalek::Keypair {
        &self.keypair
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    pub fn server_config(&self) -> &quinn::ServerConfig {
        &self.quinn_server_config
    }

    pub fn client_config(&self) -> &quinn::ClientConfig {
        &self.quinn_client_config
    }

    #[cfg(test)]
    pub(crate) fn random(server_name: &str) -> Self {
        let mut rng = rand::thread_rng();
        let keypair = ed25519_dalek::Keypair::generate(&mut rng);

        let mut builder = Self::builder();
        builder.keypair = Some(keypair);
        builder.server_name = Some(server_name.to_owned());

        builder.build().unwrap()
    }
}

/// This type provides serialized bytes for a private key.
///
/// The private key must be DER-encoded ASN.1 in either
/// PKCS#8 or PKCS#1 format.
// TODO: move this to rccheck?
trait ToPKCS8 {
    fn to_pkcs8_bytes(&self) -> Vec<u8>;
}

impl ToPKCS8 for ed25519_dalek::Keypair {
    fn to_pkcs8_bytes(&self) -> Vec<u8> {
        let kpb = ed25519::KeypairBytes {
            secret_key: self.secret.to_bytes(),
            public_key: None,
        };
        let pkcs8 = kpb.to_pkcs8_der().unwrap();
        pkcs8.as_bytes().to_vec()
    }
}

/// An error that occured when generating the TLS certificate.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
struct CertificateGenerationError(
    // Though there are multiple different errors that could occur by the code, since we are
    // generating a certificate, they should only really occur due to buggy implementations. As
    // such, we don't attempt to expose more detail than 'something went wrong', which will
    // hopefully be enough for someone to file a bug report...
    Box<dyn std::error::Error + Send + Sync>,
);

/// Configuration errors.
#[derive(Debug, thiserror::Error)]
enum ConfigError {
    #[error("An error occurred when generating the TLS certificate")]
    CertificateGeneration(#[from] CertificateGenerationError),

    #[error("An error occurred within rustls")]
    Rustls(#[from] rustls::Error),

    #[error("An error occurred generating client config certificates")]
    Webpki,
}

fn keypair_to_certificate(
    subject_names: impl Into<Vec<String>>,
    kp: ed25519_dalek::Keypair,
) -> Result<rustls::Certificate, anyhow::Error> {
    let keypair_bytes = dalek_to_keypair_bytes(kp);
    let (pkcs_bytes, alg) =
        keypair_bytes_to_pkcs8_n_algo(keypair_bytes).map_err(anyhow::Error::new)?;

    let certificate = gen_certificate(subject_names, (pkcs_bytes.as_bytes(), alg))?;
    Ok(certificate)
}

fn dalek_to_keypair_bytes(dalek_kp: ed25519_dalek::Keypair) -> ed25519::KeypairBytes {
    let private = dalek_kp.secret;
    let _public = dalek_kp.public;

    ed25519::KeypairBytes {
        secret_key: private.to_bytes(),
        // ring cannot handle the optional public key that would be legal der here
        // that is, ring expects PKCS#8 v.1
        public_key: None, // Some(_public.to_bytes()),
    }
}

fn keypair_bytes_to_pkcs8_n_algo(
    kpb: ed25519::KeypairBytes,
) -> Result<(pkcs8::der::SecretDocument, &'static SignatureAlgorithm), pkcs8::Error> {
    // PKCS#8 v2 as described in [RFC 5958].
    // PKCS#8 v2 keys include an additional public key field.
    let pkcs8 = kpb.to_pkcs8_der()?;

    Ok((pkcs8, &rcgen::PKCS_ED25519))
}

fn gen_certificate(
    subject_names: impl Into<Vec<String>>,
    key_pair: (&[u8], &'static SignatureAlgorithm),
) -> Result<rustls::Certificate, anyhow::Error> {
    let kp = KeyPair::from_der_and_sign_algo(key_pair.0, key_pair.1)?;

    let mut cert_params = CertificateParams::new(subject_names);
    cert_params.key_pair = Some(kp);
    cert_params.distinguished_name = rcgen::DistinguishedName::new();
    cert_params.alg = key_pair.1;

    let cert = rcgen::Certificate::from_params(cert_params).expect(
        "unreachable! from_params should only fail if the key is incompatible with params.algo",
    );
    let cert_bytes = cert.serialize_der()?;
    Ok(rustls::Certificate(cert_bytes))
}
