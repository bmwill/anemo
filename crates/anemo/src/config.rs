use crate::{
    crypto::{CertVerifier, ExpectedCertVerifier},
    PeerId, Result,
};
use pkcs8::EncodePrivateKey;
// use ed25519::pkcs8::EncodePrivateKey;
use quinn::VarInt;
use rcgen::{CertificateParams, KeyPair, SignatureAlgorithm};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Config {
    pub quic: Option<QuicConfig>,

    //
    // ConnectionManager Configs
    //
    pub connection_manager_channel_capacity: Option<usize>,

    /// Trigger connectivity checks every interval.
    pub connectivity_check_interval_ms: Option<u64>,

    /// Maximum delay between 2 consecutive attempts to connect with a peer.
    pub max_connection_backoff_ms: Option<u64>,
    pub connection_backoff_ms: Option<u64>,
    pub max_concurrent_outstanding_connecting_connections: Option<usize>,
    pub peer_event_broadcast_channel_capacity: Option<usize>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct QuicConfig {
    pub max_concurrent_bidi_streams: Option<u64>,
    pub max_concurrent_uni_streams: Option<u64>,

    /// How long to wait to hear from a peer before timing out a connection.
    ///
    /// In the absence of any keep-alive messages, connections will be closed if they remain idle
    /// for at least this duration.
    ///
    /// If unspecified, this will default to 10,000 milliseconds.
    ///
    /// Maximum possible value is 2^62 milliseconds.
    pub max_idle_timeout_ms: Option<u64>,

    /// Interval at which to send keep-alives to maintain otherwise idle connections.
    ///
    /// Keep-alives prevent otherwise idle connections from timing out.
    ///
    /// If unspecified, this will default to `None`, disabling keep-alives.
    pub keep_alive_interval_ms: Option<u64>,
}

impl Config {
    pub(crate) fn transport_config(&self) -> quinn::TransportConfig {
        self.quic
            .as_ref()
            .map(QuicConfig::transport_config)
            .unwrap_or_default()
    }

    pub(crate) fn connection_manager_channel_capacity(&self) -> usize {
        const CONNECTION_MANAGER_CHANNEL_CAPACITY: usize = 128;

        self.connection_manager_channel_capacity
            .unwrap_or(CONNECTION_MANAGER_CHANNEL_CAPACITY)
    }

    pub(crate) fn connectivity_check_interval(&self) -> Duration {
        const CONNECTIVITY_CHECK_INTERVAL_MS: u64 = 5_000; // 5 seconds

        Duration::from_millis(
            self.connectivity_check_interval_ms
                .unwrap_or(CONNECTIVITY_CHECK_INTERVAL_MS),
        )
    }

    #[allow(unused)]
    pub(crate) fn max_connection_backoff(&self) -> Duration {
        const MAX_CONNECTION_BACKOFF_MS: u64 = 60_000; // 1 minute

        Duration::from_millis(
            self.max_connection_backoff_ms
                .unwrap_or(MAX_CONNECTION_BACKOFF_MS),
        )
    }

    #[allow(unused)]
    pub(crate) fn connection_backoff(&self) -> Duration {
        const CONNECTION_BACKOFF_MS: u64 = 10_000; // 10 seconds

        Duration::from_millis(self.connection_backoff_ms.unwrap_or(CONNECTION_BACKOFF_MS))
    }

    pub(crate) fn max_concurrent_outstanding_connecting_connections(&self) -> usize {
        const MAX_CONCURRENT_OUTSTANDING_CONNECTING_CONNECTIONS: usize = 100;

        self.max_concurrent_outstanding_connecting_connections
            .unwrap_or(MAX_CONCURRENT_OUTSTANDING_CONNECTING_CONNECTIONS)
    }

    pub(crate) fn peer_event_broadcast_channel_capacity(&self) -> usize {
        const PEER_EVENT_BROADCAST_CHANNEL_CAPACITY: usize = 128;

        self.peer_event_broadcast_channel_capacity
            .unwrap_or(PEER_EVENT_BROADCAST_CHANNEL_CAPACITY)
    }
}

impl QuicConfig {
    pub(crate) fn transport_config(&self) -> quinn::TransportConfig {
        let mut config = quinn::TransportConfig::default();

        if let Some(max) = self
            .max_concurrent_bidi_streams
            .map(|n| VarInt::try_from(n).unwrap_or(VarInt::MAX))
        {
            config.max_concurrent_bidi_streams(max);
        }

        if let Some(max) = self
            .max_concurrent_uni_streams
            .map(|n| VarInt::try_from(n).unwrap_or(VarInt::MAX))
        {
            config.max_concurrent_uni_streams(max);
        }

        if let Some(max) = self
            .max_idle_timeout_ms
            .map(|n| VarInt::try_from(n).unwrap_or(VarInt::MAX))
            .map(Into::into)
        {
            config.max_idle_timeout(Some(max));
        }

        if let Some(keep_alive_interval) = self.keep_alive_interval_ms.map(Duration::from_millis) {
            config.keep_alive_interval(Some(keep_alive_interval));
        }

        config
    }
}

#[derive(Debug, Default)]
pub struct EndpointConfigBuilder {
    /// Ed25519 Private Key
    pub private_key: Option<[u8; 32]>,

    // TODO Maybe use server name to identify the network name?
    //
    /// Note that the end-entity certificate must have the
    /// [Subject Alternative Name](https://tools.ietf.org/html/rfc6125#section-4.1)
    /// extension to describe, e.g., the valid DNS name.
    pub server_name: Option<String>,

    pub transport_config: Option<quinn::TransportConfig>,
}

impl EndpointConfigBuilder {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn server_name<T: Into<String>>(mut self, server_name: T) -> Self {
        self.server_name = Some(server_name.into());
        self
    }

    pub fn transport_config(mut self, transport_config: quinn::TransportConfig) -> Self {
        self.transport_config = Some(transport_config);
        self
    }

    pub fn private_key(mut self, private_key: [u8; 32]) -> Self {
        self.private_key = Some(private_key);
        self
    }

    #[cfg(test)]
    pub(crate) fn random_private_key(self) -> Self {
        let mut rng = rand::thread_rng();
        let mut bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rng, &mut bytes[..]);

        self.private_key(bytes)
    }

    pub fn build(self) -> Result<EndpointConfig> {
        let keypair = ed25519::KeypairBytes {
            secret_key: self.private_key.unwrap(),
            // ring cannot handle the optional public key that would be legal der here
            // that is, ring expects PKCS#8 v.1
            public_key: None,
        };

        let server_name = self.server_name.unwrap();
        let transport_config = Arc::new(self.transport_config.unwrap_or_default());

        let cert_verifier = Arc::new(CertVerifier(server_name.clone()));
        let (certificate, pkcs8_der) = Self::generate_cert(&keypair, &server_name);

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
            transport_config.clone(),
        )?;

        let peer_id = crate::crypto::peer_id_from_certificate(&certificate).unwrap();

        Ok(EndpointConfig {
            peer_id,
            certificate,
            pkcs8_der,
            quinn_server_config: server_config,
            quinn_client_config: client_config,
            server_name,
            transport_config,
        })
    }

    fn generate_cert(
        keypair: &ed25519::KeypairBytes,
        server_name: &str,
    ) -> (rustls::Certificate, rustls::PrivateKey) {
        let pkcs8 = keypair.to_pkcs8_der().unwrap();
        let key_der = rustls::PrivateKey(pkcs8.as_bytes().to_vec());
        let certificate = keypair_to_certificate(vec![server_name.to_owned()], keypair).unwrap();
        (certificate, key_der)
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
    peer_id: PeerId,
    certificate: rustls::Certificate,
    pkcs8_der: rustls::PrivateKey,
    quinn_server_config: quinn::ServerConfig,
    quinn_client_config: quinn::ClientConfig,

    // TODO Maybe use server name to identify the network name?
    //
    /// Note that the end-entity certificate must have the
    /// [Subject Alternative Name](https://tools.ietf.org/html/rfc6125#section-4.1)
    /// extension to describe, e.g., the valid DNS name.
    server_name: String,

    transport_config: Arc<quinn::TransportConfig>,
}

impl EndpointConfig {
    pub fn builder() -> EndpointConfigBuilder {
        EndpointConfigBuilder::new()
    }

    pub fn peer_id(&self) -> PeerId {
        self.peer_id
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

    pub fn client_config_with_expected_server_identity(
        &self,
        peer_id: PeerId,
    ) -> quinn::ClientConfig {
        let server_cert_verifier =
            ExpectedCertVerifier(CertVerifier(self.server_name().into()), peer_id);
        let client_crypto = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_custom_certificate_verifier(Arc::new(server_cert_verifier))
            .with_single_cert(vec![self.certificate.clone()], self.pkcs8_der.clone())
            .unwrap();

        let mut client = quinn::ClientConfig::new(Arc::new(client_crypto));
        client.transport = self.transport_config.clone();
        client
    }

    #[cfg(test)]
    pub(crate) fn random(server_name: &str) -> Self {
        Self::builder()
            .random_private_key()
            .server_name(server_name)
            .build()
            .unwrap()
    }
}

fn keypair_to_certificate(
    subject_names: impl Into<Vec<String>>,
    keypair: &ed25519::KeypairBytes,
) -> Result<rustls::Certificate, anyhow::Error> {
    let (pkcs_bytes, alg) = keypair_bytes_to_pkcs8_n_algo(keypair).map_err(anyhow::Error::new)?;

    let certificate = gen_certificate(subject_names, (pkcs_bytes.as_bytes(), alg))?;
    Ok(certificate)
}

fn keypair_bytes_to_pkcs8_n_algo(
    kpb: &ed25519::KeypairBytes,
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
