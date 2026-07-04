use std::pin::Pin;
use std::sync::Arc;

use openssl::ssl::{Ssl, SslAcceptor};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_openssl::SslStream;

use crate::config::Config;
use crate::http::{
    bad_gateway, connect_established, header_end, origin_form_header, parse_request_header,
    RequestTarget,
};
use crate::metrics::Metrics;
use crate::relay;
use crate::tls::server_acceptor;

const MAX_HEADER_SIZE: usize = 128 * 1024;

pub(crate) struct ProxyServer {
    listener: TcpListener,
    acceptor: Arc<SslAcceptor>,
    config: Config,
    metrics: Arc<Metrics>,
}

pub(crate) async fn bind(config: Config, metrics: Arc<Metrics>) -> Result<ProxyServer, String> {
    let listener = TcpListener::bind((config.proxy_host.as_str(), config.proxy_port))
        .await
        .map_err(|err| {
            format!(
                "failed to bind proxy listener {}:{}: {err}",
                config.proxy_host, config.proxy_port
            )
        })?;
    let acceptor = Arc::new(server_acceptor(&config, config.require_client_cert)?);

    Ok(ProxyServer {
        listener,
        acceptor,
        config,
        metrics,
    })
}

impl ProxyServer {
    pub(crate) async fn run(self) -> Result<(), String> {
        let Self {
            listener,
            acceptor,
            config,
            metrics,
        } = self;

        loop {
            let (stream, _) = listener
                .accept()
                .await
                .map_err(|err| format!("failed to accept proxy connection: {err}"))?;
            stream
                .set_nodelay(true)
                .map_err(|err| format!("failed to set TCP_NODELAY on proxy connection: {err}"))?;
            metrics.inc_proxy_client_connections();

            let connection_config = config.clone();
            let connection_metrics = Arc::clone(&metrics);
            let connection_acceptor = Arc::clone(&acceptor);
            tokio::spawn(async move {
                let result = serve_tls_connection(
                    stream,
                    connection_acceptor,
                    connection_config,
                    Arc::clone(&connection_metrics),
                )
                .await;

                if let Err(err) = result {
                    connection_metrics.inc_proxy_errors();
                    eprintln!("proxy connection failed: {err}");
                }
            });
        }
    }
}

async fn serve_tls_connection(
    stream: TcpStream,
    acceptor: Arc<SslAcceptor>,
    config: Config,
    metrics: Arc<Metrics>,
) -> Result<(), String> {
    let ssl = Ssl::new(acceptor.context())
        .map_err(|err| format!("failed to create proxy TLS state: {err}"))?;
    let mut stream = SslStream::new(ssl, stream)
        .map_err(|err| format!("failed to create proxy TLS stream: {err}"))?;
    Pin::new(&mut stream)
        .accept()
        .await
        .map_err(|err| format!("proxy TLS handshake failed: {err}"))?;
    metrics.inc_proxy_tls_handshakes();
    handle_proxy_connection(stream, config, metrics).await
}

async fn handle_proxy_connection<S>(
    mut stream: S,
    config: Config,
    metrics: Arc<Metrics>,
) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (header, header_len) = read_header(&mut stream, config.relay_buffer_size).await?;
    let parsed = parse_request_header(&header[..header_len])?;

    let RequestTarget::Connect { host, port } = &parsed.target else {
        return handle_absolute_form_proxy_connection(
            stream, config, metrics, header, header_len, parsed,
        )
        .await;
    };
    metrics.inc_connect_requests();

    let mut upstream = match TcpStream::connect((host.as_str(), *port)).await {
        Ok(stream) => stream,
        Err(err) => {
            stream
                .write_all(&bad_gateway(&parsed.version))
                .await
                .map_err(|write_err| {
                    format!("failed to write CONNECT bad gateway after upstream error {err}: {write_err}")
                })?;
            stream.flush().await.map_err(|flush_err| {
                format!(
                    "failed to flush CONNECT bad gateway after upstream error {err}: {flush_err}"
                )
            })?;
            return Ok(());
        }
    };
    upstream
        .set_nodelay(true)
        .map_err(|err| format!("failed to set TCP_NODELAY on upstream connection: {err}"))?;

    stream
        .write_all(&connect_established(&parsed.version))
        .await
        .map_err(|err| format!("failed to write CONNECT established response: {err}"))?;
    stream
        .flush()
        .await
        .map_err(|err| format!("failed to flush CONNECT established response: {err}"))?;

    let buffered_tunnel = &header[header_len..];
    if !buffered_tunnel.is_empty() {
        upstream
            .write_all(buffered_tunnel)
            .await
            .map_err(|err| format!("failed to forward buffered CONNECT tunnel bytes: {err}"))?;
        metrics.add_client_to_origin_bytes(
            u64::try_from(buffered_tunnel.len())
                .map_err(|err| format!("failed to count buffered CONNECT tunnel bytes: {err}"))?,
        );
    }

    relay::bidirectional(stream, upstream, config.relay_buffer_size, metrics).await
}

async fn handle_absolute_form_proxy_connection<S>(
    mut stream: S,
    config: Config,
    metrics: Arc<Metrics>,
    header: Vec<u8>,
    header_len: usize,
    parsed: crate::http::ParsedRequest,
) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let RequestTarget::Absolute { host, port, .. } = &parsed.target else {
        return Err("proxy forwarding requires an absolute-form request target".to_string());
    };

    let mut upstream = match TcpStream::connect((host.as_str(), *port)).await {
        Ok(stream) => stream,
        Err(err) => {
            stream
                .write_all(&bad_gateway(&parsed.version))
                .await
                .map_err(|write_err| {
                    format!(
                        "failed to write proxy bad gateway after upstream error {err}: {write_err}"
                    )
                })?;
            stream.flush().await.map_err(|flush_err| {
                format!("failed to flush proxy bad gateway after upstream error {err}: {flush_err}")
            })?;
            return Ok(());
        }
    };
    upstream.set_nodelay(true).map_err(|err| {
        format!("failed to set TCP_NODELAY on absolute-form upstream connection: {err}")
    })?;

    let rewritten_header = origin_form_header(&header[..header_len], &parsed);
    upstream
        .write_all(&rewritten_header)
        .await
        .map_err(|err| format!("failed to write rewritten proxy request header: {err}"))?;

    let buffered_body = &header[header_len..];
    if !buffered_body.is_empty() {
        upstream
            .write_all(buffered_body)
            .await
            .map_err(|err| format!("failed to forward buffered proxy request bytes: {err}"))?;
        metrics.add_client_to_origin_bytes(
            u64::try_from(buffered_body.len())
                .map_err(|err| format!("failed to count buffered proxy request bytes: {err}"))?,
        );
    }

    let remaining_body = parsed.content_length.saturating_sub(buffered_body.len());
    drain_request_body(
        &mut stream,
        &mut upstream,
        remaining_body,
        config.relay_buffer_size,
        &metrics,
    )
    .await?;

    relay::bidirectional(stream, upstream, config.relay_buffer_size, metrics).await
}

async fn drain_request_body<S>(
    stream: &mut S,
    upstream: &mut TcpStream,
    mut remaining: usize,
    buffer_size: usize,
    metrics: &Metrics,
) -> Result<(), String>
where
    S: AsyncRead + Unpin,
{
    let mut buffer = vec![0; buffer_size];
    while remaining > 0 {
        let read_limit = buffer.len().min(remaining);
        let read = stream
            .read(&mut buffer[..read_limit])
            .await
            .map_err(|err| format!("failed to read proxy request body: {err}"))?;
        if read == 0 {
            return Err(format!(
                "proxy connection closed with {remaining} request body bytes remaining"
            ));
        }

        upstream
            .write_all(&buffer[..read])
            .await
            .map_err(|err| format!("failed to forward proxy request body: {err}"))?;
        metrics.add_client_to_origin_bytes(
            u64::try_from(read)
                .map_err(|err| format!("failed to count proxy request body bytes: {err}"))?,
        );
        remaining -= read;
    }

    Ok(())
}

async fn read_header<S>(stream: &mut S, buffer_size: usize) -> Result<(Vec<u8>, usize), String>
where
    S: AsyncRead + Unpin,
{
    let mut buffer = Vec::with_capacity(buffer_size);
    let mut scratch = vec![0; buffer_size];

    loop {
        if let Some(header_len) = header_end(&buffer) {
            if header_len > MAX_HEADER_SIZE {
                return Err(format!(
                    "proxy request header exceeded {MAX_HEADER_SIZE} bytes"
                ));
            }
            return Ok((buffer, header_len));
        }
        if buffer.len() >= MAX_HEADER_SIZE {
            return Err(format!(
                "proxy request header exceeded {MAX_HEADER_SIZE} bytes"
            ));
        }

        let read_limit = scratch.len().min(MAX_HEADER_SIZE + 1 - buffer.len());
        let read = stream
            .read(&mut scratch[..read_limit])
            .await
            .map_err(|err| format!("failed to read proxy request header: {err}"))?;
        if read == 0 {
            return Err("proxy connection closed with incomplete request header".to_string());
        }
        buffer.extend_from_slice(&scratch[..read]);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use crate::config::Config;
    use crate::metrics::Metrics;

    fn test_config() -> Config {
        Config {
            cert_file: String::new(),
            key_file: String::new(),
            ca_file: None,
            require_client_cert: false,
            origin_tls: false,
            origin_host: "127.0.0.1".to_string(),
            origin_port: 18080,
            proxy_host: "127.0.0.1".to_string(),
            proxy_port: 18443,
            response_size: 1024,
            response_size_sequence: Vec::new(),
            relay_buffer_size: 16 * 1024,
            origin_delay_ms: 0,
            origin_close_every_n_requests: None,
        }
    }

    #[tokio::test]
    async fn forwards_absolute_form_request_as_origin_form() {
        let origin_listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let origin_addr = origin_listener.local_addr().unwrap();
        let expected_request = format!(
            "POST /path?q=1 HTTP/1.1\r\nHost: {}:{}\r\nContent-Length: 4\r\n\r\nbody",
            origin_addr.ip(),
            origin_addr.port()
        );
        let origin_expected_request = expected_request.clone();
        let origin_task = tokio::spawn(async move {
            let (mut origin, _) = origin_listener.accept().await.unwrap();
            let mut received = vec![0; origin_expected_request.len()];
            origin.read_exact(&mut received).await.unwrap();
            origin
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await
                .unwrap();
            origin.shutdown().await.unwrap();
            received
        });

        let (mut client, proxy_stream) = duplex(4096);
        let metrics = Arc::new(Metrics::default());
        let proxy_task = tokio::spawn(super::handle_proxy_connection(
            proxy_stream,
            test_config(),
            Arc::clone(&metrics),
        ));

        client
            .write_all(
                format!(
                    "POST http://{}:{}/path?q=1 HTTP/1.1\r\nHost: {}:{}\r\nContent-Length: 4\r\n\r\nbody",
                    origin_addr.ip(),
                    origin_addr.port(),
                    origin_addr.ip(),
                    origin_addr.port()
                )
                .as_bytes(),
            )
            .await
            .unwrap();

        let mut response = vec![0; 40];
        client.read_exact(&mut response).await.unwrap();
        client.shutdown().await.unwrap();

        let received = origin_task.await.unwrap();
        proxy_task.await.unwrap().unwrap();

        assert_eq!(received, expected_request.as_bytes());
        assert_eq!(response, b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        assert!(metrics
            .json_snapshot()
            .contains("\"bytes_client_to_origin\":4"));
    }
}
