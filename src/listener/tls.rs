use std::{error::Error, fs, io::BufReader};

use crate::option;

pub(super) fn new_tls_config(
    options: option::ListenerTLSOptions,
) -> Result<tokio_rustls::rustls::ServerConfig, Box<dyn Error + Send + Sync>> {
    let config_builder = tokio_rustls::rustls::ServerConfig::builder();

    let mut root_store = tokio_rustls::rustls::RootCertStore::empty();
    if let Some(ca_files) = options.client_ca_file {
        let ca_files = ca_files.into_list();
        for f in ca_files {
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
        return Err("tls: missing server-cert-file".into());
    }
    let mut cert_reader = fs::File::open(&options.server_cert_file)
        .map(|f| BufReader::new(f))
        .map_err(|err| {
            format!(
                "tls: failed to open server-cert-file {}: {}",
                options.server_cert_file, err
            )
        })?;
    let mut certs = Vec::new();
    for cert in rustls_pemfile::certs(&mut cert_reader) {
        match cert {
            Ok(cert) => certs.push(cert),
            Err(e) => {
                return Err(format!(
                    "tls: failed to parse server-cert-file {}: {}",
                    options.server_cert_file, e
                )
                .into());
            }
        }
    }
    drop(cert_reader);
    if options.server_key_file.is_empty() {
        return Err("tls: missing server-key-file".into());
    }
    let mut key_reader = fs::File::open(&options.server_key_file)
        .map(|f| BufReader::new(f))
        .map_err(|err| {
            format!(
                "tls: failed to open server-key-file {}: {}",
                options.server_key_file, err
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
        return Err(format!(
            "tls: failed to parse server-key-file {}: no private key found",
            options.server_key_file
        )
        .into());
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
        .map_err(|err| format!("tls: failed to create tls config with certificate: {}", err))?;
    Ok(config)
}
