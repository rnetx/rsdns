use std::{error::Error, fs, io::BufReader};

use crate::option;

pub(super) fn new_tls_config(
    options: option::UpstreamTLSOptions,
) -> Result<(tokio_rustls::rustls::ClientConfig, Option<String>), Box<dyn Error + Send + Sync>> {
    let config_builder = tokio_rustls::rustls::ClientConfig::builder();

    let mut root_store = tokio_rustls::rustls::RootCertStore::empty();
    if options.server_ca_file.len() > 0 {
        for f in options.server_ca_file.into_list() {
            let file = fs::File::open(&f)
                .map_err(|err| format!("tls: failed to open server-ca-file {}: {}", f, err))?;
            let mut reader = BufReader::new(file);
            let mut cer_list = Vec::new();
            for cer in rustls_pemfile::certs(&mut reader) {
                match cer {
                    Ok(cert) => cer_list.push(cert),
                    Err(e) => {
                        return Err(
                            format!("tls: failed to parse server-ca-file {}: {}", f, e).into()
                        );
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

    let config_builder = config_builder
        .with_safe_defaults()
        .with_root_certificates(root_store);

    let config = match (options.client_cert_file, options.client_key_file) {
        (Some(cert_file), Some(key_file)) => {
            let mut cert_reader = fs::File::open(&cert_file)
                .map(|f| BufReader::new(f))
                .map_err(|err| {
                    format!(
                        "tls: failed to open client-cert-file {}: {}",
                        cert_file, err
                    )
                })?;
            let mut certs = Vec::new();
            for cert in rustls_pemfile::certs(&mut cert_reader) {
                match cert {
                    Ok(cert) => certs.push(cert),
                    Err(e) => {
                        return Err(format!(
                            "tls: failed to parse client-cert-file {}: {}",
                            cert_file, e
                        )
                        .into());
                    }
                }
            }
            drop(cert_reader);
            let mut key_reader = fs::File::open(&key_file)
                .map(|f| BufReader::new(f))
                .map_err(|err| {
                    format!("tls: failed to open client-key-file {}: {}", key_file, err)
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
                return Err(format!(
                    "tls: failed to parse client-key-file {}: no private key found",
                    key_file
                )
                .into());
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
                    format!("tls: failed to create tls config with client auth: {}", err)
                })?
        }
        (None, Some(_)) => {
            return Err("tls: missing client-cert-file".into());
        }
        (Some(_), None) => {
            return Err("tls: missing client-key-file".into());
        }
        (None, None) => config_builder.with_no_client_auth(),
    };

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
