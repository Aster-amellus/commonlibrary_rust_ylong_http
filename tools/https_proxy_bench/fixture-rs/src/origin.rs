use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use openssl::ssl::{Ssl, SslAcceptor};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_openssl::SslStream;

use crate::config::Config;
use crate::http::{header_end, parse_request_header, response_header};
use crate::metrics::Metrics;
use crate::tls::server_acceptor;

const MAX_HEADER_SIZE: usize = 128 * 1024;

pub(crate) struct OriginServer {
    listener: TcpListener,
    acceptor: Option<Arc<SslAcceptor>>,
    config: Config,
    metrics: Arc<Metrics>,
}

pub(crate) async fn bind(config: Config, metrics: Arc<Metrics>) -> Result<OriginServer, String> {
    let listener = TcpListener::bind((config.origin_host.as_str(), config.origin_port))
        .await
        .map_err(|err| {
            format!(
                "failed to bind origin listener {}:{}: {err}",
                config.origin_host, config.origin_port
            )
        })?;
    let acceptor = if config.origin_tls {
        Some(Arc::new(server_acceptor(&config, false)?))
    } else {
        None
    };

    Ok(OriginServer {
        listener,
        acceptor,
        config,
        metrics,
    })
}

impl OriginServer {
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
                .map_err(|err| format!("failed to accept origin connection: {err}"))?;
            stream
                .set_nodelay(true)
                .map_err(|err| format!("failed to set TCP_NODELAY on origin connection: {err}"))?;
            metrics.inc_origin_tcp_connections();

            let connection_config = config.clone();
            let connection_metrics = Arc::clone(&metrics);
            let connection_acceptor = acceptor.clone();
            tokio::spawn(async move {
                let result = if let Some(acceptor) = connection_acceptor {
                    serve_tls_connection(
                        stream,
                        acceptor,
                        connection_config,
                        Arc::clone(&connection_metrics),
                    )
                    .await
                } else {
                    serve_connection(stream, connection_config, Arc::clone(&connection_metrics))
                        .await
                };

                if let Err(err) = result {
                    connection_metrics.inc_origin_errors();
                    eprintln!("origin connection failed: {err}");
                }
            });
        }
    }
}

async fn serve_connection<S>(
    mut stream: S,
    config: Config,
    metrics: Arc<Metrics>,
) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let delay = Duration::from_millis(config.origin_delay_ms);
    let mut buffer = Vec::with_capacity(config.relay_buffer_size);
    let mut scratch = vec![0; config.relay_buffer_size];
    let mut request_index = 0usize;

    loop {
        let header_len = loop {
            if let Some(header_len) = header_end(&buffer) {
                if header_len > MAX_HEADER_SIZE {
                    return Err(format!(
                        "origin request header exceeded {MAX_HEADER_SIZE} bytes"
                    ));
                }
                break header_len;
            }
            if buffer.len() > MAX_HEADER_SIZE {
                return Err(format!(
                    "origin request header exceeded {MAX_HEADER_SIZE} bytes"
                ));
            }

            let read = stream
                .read(&mut scratch)
                .await
                .map_err(|err| format!("failed to read origin request header: {err}"))?;
            if read == 0 {
                if buffer.is_empty() {
                    return Ok(());
                }
                return Err("origin connection closed with incomplete request header".to_string());
            }
            buffer.extend_from_slice(&scratch[..read]);
        };

        let parsed = parse_request_header(&buffer[..header_len])?;
        let request_len = header_len
            .checked_add(parsed.content_length)
            .ok_or_else(|| "origin request length overflow".to_string())?;

        while buffer.len() < request_len {
            let read = stream
                .read(&mut scratch)
                .await
                .map_err(|err| format!("failed to read origin request body: {err}"))?;
            if read == 0 {
                return Err("origin connection closed with incomplete request body".to_string());
            }
            buffer.extend_from_slice(&scratch[..read]);
        }

        buffer.drain(..request_len);

        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }

        let response_size = response_size_for_request(&config, request_index);
        let body = vec![b'x'; response_size];
        let response_header = response_header("HTTP/1.1", "200 OK", body.len());
        stream
            .write_all(&response_header)
            .await
            .map_err(|err| format!("failed to write origin response header: {err}"))?;
        stream
            .write_all(&body)
            .await
            .map_err(|err| format!("failed to write origin response body: {err}"))?;
        stream
            .flush()
            .await
            .map_err(|err| format!("failed to flush origin response: {err}"))?;
        metrics.record_origin_response_size(response_size);
        metrics.inc_forwarded_requests();

        request_index = request_index
            .checked_add(1)
            .ok_or_else(|| "origin request index overflow".to_string())?;
        if let Some(every) = config.origin_close_every_n_requests {
            if request_index % every == 0 {
                metrics.inc_origin_policy_closes();
                break;
            }
        }
    }

    Ok(())
}

fn response_size_for_request(config: &Config, request_index: usize) -> usize {
    if config.response_size_sequence.is_empty() {
        config.response_size
    } else {
        config.response_size_sequence[request_index % config.response_size_sequence.len()]
    }
}

async fn serve_tls_connection(
    stream: TcpStream,
    acceptor: Arc<openssl::ssl::SslAcceptor>,
    config: Config,
    metrics: Arc<Metrics>,
) -> Result<(), String> {
    let ssl = Ssl::new(acceptor.context())
        .map_err(|err| format!("failed to create origin TLS state: {err}"))?;
    let mut stream = SslStream::new(ssl, stream)
        .map_err(|err| format!("failed to create origin TLS stream: {err}"))?;
    Pin::new(&mut stream)
        .accept()
        .await
        .map_err(|err| format!("origin TLS handshake failed: {err}"))?;
    metrics.inc_origin_tls_handshakes();
    serve_connection(stream, config, metrics).await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

    use super::serve_connection;
    use crate::config::Config;
    use crate::metrics::Metrics;

    fn test_config() -> Config {
        Config {
            cert_file: "server.pem".to_string(),
            key_file: "server.key".to_string(),
            ca_file: None,
            require_client_cert: false,
            origin_tls: false,
            origin_host: "127.0.0.1".to_string(),
            origin_port: 18080,
            proxy_host: "127.0.0.1".to_string(),
            proxy_port: 18443,
            response_size: 4,
            response_size_sequence: Vec::new(),
            relay_buffer_size: 16 * 1024,
            origin_delay_ms: 0,
            origin_close_every_n_requests: None,
        }
    }

    #[tokio::test]
    async fn serves_pipelined_requests_after_draining_body() {
        let (client, server) = duplex(4096);
        let config = test_config();
        let metrics = Arc::new(Metrics::default());
        let server_metrics = Arc::clone(&metrics);

        let server_task = tokio::spawn(async move {
            serve_connection(server, config, server_metrics)
                .await
                .unwrap();
        });

        let mut client = client;
        client
            .write_all(
                b"POST /one HTTP/1.1\r\nHost: local\r\nContent-Length: 5\r\n\r\nabcdeGET /two HTTP/1.1\r\nHost: local\r\n\r\n",
            )
            .await
            .unwrap();

        let expected = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: keep-alive\r\n\r\nxxxxHTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: keep-alive\r\n\r\nxxxx";
        let mut response = vec![0; expected.len()];
        client.read_exact(&mut response).await.unwrap();

        assert_eq!(response, expected);
        assert!(metrics.json_snapshot().contains("\"forwarded_requests\":2"));

        drop(client);
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn serves_configured_response_size_sequence() {
        let (client, server) = duplex(4096);
        let mut config = test_config();
        config.response_size_sequence = vec![1, 3];
        let metrics = Arc::new(Metrics::default());
        let server_metrics = Arc::clone(&metrics);

        let server_task = tokio::spawn(async move {
            serve_connection(server, config, server_metrics)
                .await
                .unwrap();
        });

        let mut client = client;
        client
            .write_all(
                b"GET /one HTTP/1.1\r\nHost: local\r\n\r\nGET /two HTTP/1.1\r\nHost: local\r\n\r\n",
            )
            .await
            .unwrap();

        let expected = b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: keep-alive\r\n\r\nxHTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: keep-alive\r\n\r\nxxx";
        let mut response = vec![0; expected.len()];
        client.read_exact(&mut response).await.unwrap();

        assert_eq!(response, expected);
        assert!(metrics
            .json_snapshot()
            .contains("\"origin_small_responses\":2"));

        drop(client);
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn closes_origin_connection_by_policy() {
        let (client, server) = duplex(4096);
        let mut config = test_config();
        config.origin_close_every_n_requests = Some(1);
        let metrics = Arc::new(Metrics::default());
        let server_metrics = Arc::clone(&metrics);

        let server_task = tokio::spawn(async move {
            serve_connection(server, config, server_metrics)
                .await
                .unwrap();
        });

        let mut client = client;
        client
            .write_all(b"GET /one HTTP/1.1\r\nHost: local\r\n\r\n")
            .await
            .unwrap();

        let expected =
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: keep-alive\r\n\r\nxxxx";
        let mut response = vec![0; expected.len()];
        client.read_exact(&mut response).await.unwrap();

        let mut eof = [0u8; 1];
        let read = client.read(&mut eof).await.unwrap();

        assert_eq!(read, 0);
        assert!(metrics
            .json_snapshot()
            .contains("\"origin_policy_closes\":1"));

        server_task.await.unwrap();
    }
}
