use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
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

struct PreparedResponses {
    responses: Vec<PreparedResponse>,
}

#[derive(Clone)]
struct PreparedResponse {
    header: Vec<u8>,
    body: Bytes,
}

impl PreparedResponses {
    fn new(config: &Config) -> Self {
        let sizes = if config.response_size_sequence.is_empty() {
            vec![config.response_size]
        } else {
            config.response_size_sequence.clone()
        };
        let responses = sizes.into_iter().map(PreparedResponse::new).collect();
        Self { responses }
    }

    fn get(&self, request_index: usize) -> &PreparedResponse {
        &self.responses[request_index % self.responses.len()]
    }
}

impl PreparedResponse {
    fn new(size: usize) -> Self {
        let body = Bytes::from(vec![b'x'; size]);
        let header = response_header("HTTP/1.1", "200 OK", body.len());
        Self { header, body }
    }

    fn header(&self) -> &[u8] {
        &self.header
    }

    fn body(&self) -> &[u8] {
        &self.body
    }

    fn body_bytes(&self) -> Bytes {
        self.body.clone()
    }

    fn body_len(&self) -> usize {
        self.body.len()
    }
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
        Some(Arc::new(server_acceptor(
            &config,
            false,
            config.origin_http2,
        )?))
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
    // PERF: benchmark responses are deterministic; prebuilding them avoids
    // allocator and memset noise in the fixture's per-request hot path.
    let responses = PreparedResponses::new(&config);
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

        let response = responses.get(request_index);
        stream
            .write_all(response.header())
            .await
            .map_err(|err| format!("failed to write origin response header: {err}"))?;
        stream
            .write_all(response.body())
            .await
            .map_err(|err| format!("failed to write origin response body: {err}"))?;
        stream
            .flush()
            .await
            .map_err(|err| format!("failed to flush origin response: {err}"))?;
        metrics.record_origin_response_size(response.body_len());
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

async fn serve_h2_connection<S>(
    stream: S,
    config: Config,
    metrics: Arc<Metrics>,
) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let delay = Duration::from_millis(config.origin_delay_ms);
    let responses = Arc::new(PreparedResponses::new(&config));
    let mut connection = h2::server::handshake(stream)
        .await
        .map_err(|err| format!("origin HTTP/2 handshake failed: {err}"))?;
    let request_index = Arc::new(AtomicUsize::new(0));

    while let Some(request) = connection.accept().await {
        let (request, mut respond) =
            request.map_err(|err| format!("failed to accept origin HTTP/2 stream: {err}"))?;
        // Relaxed is enough: the counter only assigns a unique response
        // sequence slot and does not publish data between stream tasks.
        let current_index = request_index.fetch_add(1, Ordering::Relaxed);
        let response = responses.get(current_index).clone();
        let stream_metrics = Arc::clone(&metrics);
        tokio::spawn(async move {
            if let Err(err) = serve_h2_stream(
                request,
                &mut respond,
                response,
                delay,
                Arc::clone(&stream_metrics),
            )
            .await
            {
                stream_metrics.inc_origin_errors();
                eprintln!("origin HTTP/2 stream failed: {err}");
            }
        });

        let handled_requests = current_index
            .checked_add(1)
            .ok_or_else(|| "origin request index overflow".to_string())?;
        if let Some(every) = config.origin_close_every_n_requests {
            if handled_requests % every == 0 {
                metrics.inc_origin_policy_closes();
                break;
            }
        }
    }

    Ok(())
}

async fn serve_h2_stream(
    request: ::http::Request<h2::RecvStream>,
    respond: &mut h2::server::SendResponse<Bytes>,
    response: PreparedResponse,
    delay: Duration,
    metrics: Arc<Metrics>,
) -> Result<(), String> {
    let mut body = request.into_body();
    while let Some(chunk) = body.data().await {
        let chunk = chunk.map_err(|err| format!("failed to read origin HTTP/2 body: {err}"))?;
        body.flow_control()
            .release_capacity(chunk.len())
            .map_err(|err| format!("failed to release origin HTTP/2 body capacity: {err}"))?;
    }

    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }

    let response_head = ::http::Response::builder()
        .status(::http::StatusCode::OK)
        .header("content-length", response.body_len().to_string())
        .body(())
        .map_err(|err| format!("failed to build origin HTTP/2 response: {err}"))?;
    let end_stream = response.body_len() == 0;
    let mut send = respond
        .send_response(response_head, end_stream)
        .map_err(|err| format!("failed to send origin HTTP/2 response head: {err}"))?;
    if !end_stream {
        send.send_data(response.body_bytes(), true)
            .map_err(|err| format!("failed to send origin HTTP/2 response body: {err}"))?;
    }

    metrics.record_origin_response_size(response.body_len());
    metrics.inc_forwarded_requests();

    Ok(())
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
    if config.origin_http2 && stream.ssl().selected_alpn_protocol() == Some(b"h2") {
        serve_h2_connection(stream, config, metrics).await
    } else {
        serve_connection(stream, config, metrics).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use bytes::Bytes;
    use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

    use super::{serve_connection, serve_h2_connection, PreparedResponses};
    use crate::config::Config;
    use crate::metrics::Metrics;

    fn test_config() -> Config {
        Config {
            cert_file: "server.pem".to_string(),
            key_file: "server.key".to_string(),
            ca_file: None,
            require_client_cert: false,
            origin_tls: false,
            origin_http2: false,
            origin_host: "127.0.0.1".to_string(),
            origin_port: 18080,
            proxy_host: "127.0.0.1".to_string(),
            proxy_port: 18443,
            response_size: 4,
            response_size_sequence: Vec::new(),
            relay_buffer_size: 16 * 1024,
            origin_delay_ms: 0,
            origin_close_every_n_requests: None,
            tls_groups: None,
        }
    }

    #[test]
    fn prepares_single_response_from_default_size() {
        let mut config = test_config();
        config.response_size = 7;

        let responses = PreparedResponses::new(&config);
        let response = responses.get(0);

        assert_eq!(response.body(), b"xxxxxxx");
        assert_eq!(
            response.header(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: keep-alive\r\n\r\n"
        );
        assert_eq!(responses.get(3).body_len(), 7);
    }

    #[test]
    fn prepared_responses_cycle_configured_size_sequence() {
        let mut config = test_config();
        config.response_size_sequence = vec![1, 3];

        let responses = PreparedResponses::new(&config);

        assert_eq!(responses.get(0).body(), b"x");
        assert_eq!(responses.get(1).body(), b"xxx");
        assert_eq!(responses.get(2).body(), b"x");
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

    #[tokio::test]
    async fn serves_http2_response() {
        let (client, server) = duplex(4096);
        let mut config = test_config();
        config.origin_tls = true;
        config.origin_http2 = true;
        config.response_size = 5;
        let metrics = Arc::new(Metrics::default());
        let server_metrics = Arc::clone(&metrics);

        let server_task = tokio::spawn(async move {
            serve_h2_connection(server, config, server_metrics)
                .await
                .unwrap();
        });

        let (mut client, connection) = h2::client::handshake(client).await.unwrap();
        let driver = tokio::spawn(async move { connection.await });
        let request = ::http::Request::builder()
            .uri("https://127.0.0.1:18080/")
            .body(())
            .unwrap();
        let (response, send_stream) = client.send_request(request, true).unwrap();
        let response = response.await.unwrap();

        assert_eq!(response.status(), ::http::StatusCode::OK);
        let mut body = response.into_body();
        let chunk = body.data().await.unwrap().unwrap();
        assert_eq!(chunk, Bytes::from_static(b"xxxxx"));
        assert!(body.data().await.is_none());
        assert!(metrics.json_snapshot().contains("\"forwarded_requests\":1"));

        drop(body);
        drop(send_stream);
        drop(client);
        let _ = driver.await.unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn serves_http2_streams_while_another_request_body_is_open() {
        let (client, server) = duplex(4096);
        let mut config = test_config();
        config.origin_tls = true;
        config.origin_http2 = true;
        let metrics = Arc::new(Metrics::default());
        let server_metrics = Arc::clone(&metrics);

        let server_task = tokio::spawn(async move {
            serve_h2_connection(server, config, server_metrics)
                .await
                .unwrap();
        });

        let (mut client, connection) = h2::client::handshake(client).await.unwrap();
        let driver = tokio::spawn(async move { connection.await });
        let open_body_request = ::http::Request::builder()
            .uri("https://127.0.0.1:18080/open")
            .body(())
            .unwrap();
        let (blocked_response, mut blocked_send_stream) =
            client.send_request(open_body_request, false).unwrap();

        let ready_request = ::http::Request::builder()
            .uri("https://127.0.0.1:18080/ready")
            .body(())
            .unwrap();
        let (ready_response, ready_send_stream) = client.send_request(ready_request, true).unwrap();
        let ready_response = tokio::time::timeout(Duration::from_millis(200), ready_response)
            .await
            .expect("second HTTP/2 stream should not wait for the first stream body")
            .unwrap();

        assert_eq!(ready_response.status(), ::http::StatusCode::OK);
        assert!(metrics.json_snapshot().contains("\"forwarded_requests\":1"));

        blocked_send_stream.send_data(Bytes::new(), true).unwrap();
        let blocked_response = blocked_response.await.unwrap();
        assert_eq!(blocked_response.status(), ::http::StatusCode::OK);

        drop(ready_response);
        drop(ready_send_stream);
        drop(blocked_response);
        drop(blocked_send_stream);
        drop(client);
        let _ = driver.await.unwrap();
        server_task.await.unwrap();
    }
}
