use std::{fs, io::BufReader, sync::Arc};

use crate::option;

pub(super) fn new_tls_config(
    options: option::UpstreamTLSOptions,
) -> anyhow::Result<(tokio_rustls::rustls::ClientConfig, Option<String>)> {
    let config_builder = tokio_rustls::rustls::ClientConfig::builder();

    let mut root_store = tokio_rustls::rustls::RootCertStore::empty();
    if !options.insecure {
        if options.server_ca_file.len() > 0 {
            for f in options.server_ca_file {
                let file = fs::File::open(&f).map_err(|err| {
                    anyhow::anyhow!("tls: failed to open server-ca-file {}: {}", f, err)
                })?;
                let mut reader = BufReader::new(file);
                let mut cer_list = Vec::new();
                for cer in rustls_pemfile::certs(&mut reader) {
                    match cer {
                        Ok(cert) => cer_list.push(cert),
                        Err(e) => {
                            return Err(anyhow::anyhow!(
                                "tls: failed to parse server-ca-file {}: {}",
                                f,
                                e
                            ));
                        }
                    }
                }
                root_store.add_parsable_certificates(&cer_list);
            }
        } else {
            root_store.add_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.iter().map(|r| {
                tokio_rustls::rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
                    r.subject.as_ref(),
                    r.subject_public_key_info.as_ref(),
                    r.name_constraints.as_ref().map(|r| r.as_ref().to_vec()),
                )
            }));
        }
    }

    let config_builder = config_builder
        .with_safe_defaults()
        .with_root_certificates(root_store);

    let mut config = match (options.client_cert_file, options.client_key_file) {
        (Some(cert_file), Some(key_file)) => {
            let mut cert_reader = fs::File::open(&cert_file)
                .map(|f| BufReader::new(f))
                .map_err(|err| {
                    anyhow::anyhow!(
                        "tls: failed to open client-cert-file {}: {}",
                        cert_file,
                        err
                    )
                })?;
            let mut certs = Vec::new();
            for cert in rustls_pemfile::certs(&mut cert_reader) {
                match cert {
                    Ok(cert) => certs.push(cert),
                    Err(e) => {
                        return Err(anyhow::anyhow!(
                            "tls: failed to parse client-cert-file {}: {}",
                            cert_file,
                            e
                        ));
                    }
                }
            }
            drop(cert_reader);
            let mut key_reader = fs::File::open(&key_file)
                .map(|f| BufReader::new(f))
                .map_err(|err| {
                    anyhow::anyhow!("tls: failed to open client-key-file {}: {}", key_file, err)
                })?;
            let mut key: Option<rustls_pki_types::PrivateKeyDer> = None;
            loop {
                if let Ok(res) = rustls_pemfile::read_one(&mut key_reader) {
                    match res {
                        Some(rustls_pemfile::Item::Pkcs1Key(k)) => break key = Some(k.into()),
                        Some(rustls_pemfile::Item::Pkcs8Key(k)) => break key = Some(k.into()),
                        Some(rustls_pemfile::Item::Sec1Key(k)) => break key = Some(k.into()),
                        None => break,
                        _ => {}
                    }
                }
            }
            if key.is_none() {
                return Err(anyhow::anyhow!(
                    "tls: failed to parse client-key-file {}: no private key found",
                    key_file
                ));
            }
            let key = key.unwrap();
            config_builder
                .with_client_auth_cert(
                    certs
                        .into_iter()
                        .map(|cert| tokio_rustls::rustls::Certificate(cert.as_ref().to_vec()))
                        .collect(),
                    tokio_rustls::rustls::PrivateKey(key.secret_der().to_vec()),
                )
                .map_err(|err| {
                    anyhow::anyhow!("tls: failed to create tls config with client auth: {}", err)
                })?
        }
        (None, Some(_)) => {
            return Err(anyhow::anyhow!("tls: missing client-cert-file"));
        }
        (Some(_), None) => {
            return Err(anyhow::anyhow!("tls: missing client-key-file"));
        }
        (None, None) => config_builder.with_no_client_auth(),
    };

    if options.insecure {
        let mut dangerous_config = rustls::client::DangerousClientConfig { cfg: &mut config };
        dangerous_config.set_certificate_verifier(Arc::new(DangerousServerVerifier));
    }

    let server_name = match options.server_name {
        Some(s) => {
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        }
        None => None,
    };

    Ok((config, server_name))
}

pub(crate) struct DangerousServerVerifier;

impl rustls::client::ServerCertVerifier for DangerousServerVerifier {
    fn verify_server_cert(
        &self,
        _: &rustls::Certificate,
        _: &[rustls::Certificate],
        _: &rustls::ServerName,
        _: &mut dyn Iterator<Item = &[u8]>,
        _: &[u8],
        _: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::Certificate,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls::Certificate,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::HandshakeSignatureValid::assertion())
    }
}
