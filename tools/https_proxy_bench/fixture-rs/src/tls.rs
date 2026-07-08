use openssl::ssl::{
    select_next_proto, AlpnError, SslAcceptor, SslFiletype, SslMethod, SslVerifyMode,
};

use crate::config::Config;

pub(crate) fn server_acceptor(
    config: &Config,
    require_client_cert: bool,
    enable_h2_alpn: bool,
) -> Result<SslAcceptor, String> {
    let mut builder = SslAcceptor::mozilla_intermediate(SslMethod::tls())
        .map_err(|e| format!("failed to create TLS acceptor: {e}"))?;
    builder
        .set_private_key_file(&config.key_file, SslFiletype::PEM)
        .map_err(|e| format!("failed to load private key {}: {e}", config.key_file))?;
    builder
        .set_certificate_chain_file(&config.cert_file)
        .map_err(|e| format!("failed to load certificate {}: {e}", config.cert_file))?;
    builder
        .check_private_key()
        .map_err(|e| format!("certificate and private key do not match: {e}"))?;

    if let Some(groups) = &config.tls_groups {
        builder
            .set_groups_list(groups)
            .map_err(|e| format!("failed to set TLS groups {groups}: {e}"))?;
    }
    if enable_h2_alpn {
        builder.set_alpn_select_callback(|_, client| {
            select_next_proto(b"\x02h2\x08http/1.1", client).ok_or(AlpnError::NOACK)
        });
    }

    if require_client_cert {
        let ca_file = config
            .ca_file
            .as_ref()
            .ok_or_else(|| "client certificate verification requires --ca-file".to_string())?;
        builder
            .set_ca_file(ca_file)
            .map_err(|e| format!("failed to load client CA {ca_file}: {e}"))?;
        builder.set_verify(SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT);
    }

    Ok(builder.build())
}
