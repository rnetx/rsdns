use std::{fs, io::BufReader};

use crate::option;

pub(super) fn new_tls_config(
    options: option::ListenerTLSOptions,
) -> anyhow::Result<tokio_rustls::rustls::ServerConfig> {
    let config_builder = tokio_rustls::rustls::ServerConfig::builder();

    let mut root_store = tokio_rustls::rustls::RootCertStore::empty();
    for f in options.client_ca_file {
        let file = fs::File::open(&f)
            .map_err(|err| anyhow::anyhow!("tls: failed to open server-ca-file {}: {}", f, err))?;
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

    let config_builder = config_builder
        .with_safe_defaults()
        .with_client_cert_verifier({
            if root_store.is_empty() {
                tokio_rustls::rustls::server::NoClientAuth::boxed()
            } else {
                tokio_rustls::rustls::server::AllowAnyAuthenticatedClient::new(root_store).boxed()
            }
        });

    if options.server_cert_file.is_empty() {
        return Err(anyhow::anyhow!("tls: missing server-cert-file"));
    }
    let mut cert_reader = fs::File::open(&options.server_cert_file)
        .map(|f| BufReader::new(f))
        .map_err(|err| {
            anyhow::anyhow!(
                "tls: failed to open server-cert-file {}: {}",
                options.server_cert_file,
                err
            )
        })?;
    let mut certs = Vec::new();
    for cert in rustls_pemfile::certs(&mut cert_reader) {
        match cert {
            Ok(cert) => certs.push(cert),
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "tls: failed to parse server-cert-file {}: {}",
                    options.server_cert_file,
                    e
                ));
            }
        }
    }
    drop(cert_reader);
    if options.server_key_file.is_empty() {
        return Err(anyhow::anyhow!("tls: missing server-key-file"));
    }
    let mut key_reader = fs::File::open(&options.server_key_file)
        .map(|f| BufReader::new(f))
        .map_err(|err| {
            anyhow::anyhow!(
                "tls: failed to open server-key-file {}: {}",
                options.server_key_file,
                err
            )
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
            "tls: failed to parse server-key-file {}: no private key found",
            options.server_key_file
        ));
    }
    let key = key.unwrap();
    let config = config_builder
        .with_single_cert(
            certs
                .into_iter()
                .map(|cert| tokio_rustls::rustls::Certificate(cert.as_ref().to_vec()))
                .collect(),
            tokio_rustls::rustls::PrivateKey(key.secret_der().to_vec()),
        )
        .map_err(|err| {
            anyhow::anyhow!("tls: failed to create tls config with certificate: {}", err)
        })?;
    Ok(config)
}
