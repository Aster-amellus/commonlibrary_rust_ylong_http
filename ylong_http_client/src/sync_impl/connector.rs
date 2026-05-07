// Copyright (c) 2023 Huawei Device Co., Ltd.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::io::{Read, Write};

use ylong_http::request::uri::Uri;

use crate::util::config::ConnectorConfig;

/// `Connector` trait used by `Client`. `Connector` provides synchronous
/// connection establishment interfaces.
pub trait Connector {
    /// The connection object established by `Connector::connect`.
    type Stream: Read + Write + 'static;
    /// Possible errors during connection establishment.
    type Error: Into<Box<dyn std::error::Error + Send + Sync>>;

    /// Attempts to establish a synchronous connection.
    fn connect(&self, uri: &Uri) -> Result<Self::Stream, Self::Error>;
}

/// Connector for creating HTTP connections synchronously.
///
/// `HttpConnector` implements `sync_impl::Connector` trait.
pub struct HttpConnector {
    config: ConnectorConfig,
}

impl HttpConnector {
    /// Creates a new `HttpConnector`.
    pub(crate) fn new(config: ConnectorConfig) -> HttpConnector {
        HttpConnector { config }
    }
}

impl Default for HttpConnector {
    fn default() -> Self {
        Self::new(ConnectorConfig::default())
    }
}

#[cfg(not(feature = "__tls"))]
pub mod no_tls {
    use std::io::Error;
    use std::net::TcpStream;

    use ylong_http::request::uri::Uri;

    use crate::sync_impl::Connector;

    impl Connector for super::HttpConnector {
        type Stream = TcpStream;
        type Error = Error;

        fn connect(&self, uri: &Uri) -> Result<Self::Stream, Self::Error> {
            let addr = if let Some(proxy) = self.config.proxies.match_proxy(uri) {
                proxy.via_proxy(uri).authority().unwrap().to_string()
            } else {
                uri.authority().unwrap().to_string()
            };
            TcpStream::connect(addr)
        }
    }
}

#[cfg(feature = "__tls")]
pub mod tls_conn {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    use ylong_http::request::uri::{Scheme, Uri};

    use crate::sync_impl::{Connector, MixStream};
    use crate::util::c_openssl::ssl::SslStream;
    use crate::{ErrorKind, HttpClientError, TlsConfig};

    impl Connector for super::HttpConnector {
        type Stream = MixStream<TcpStream>;
        type Error = HttpClientError;

        fn connect(&self, uri: &Uri) -> Result<Self::Stream, Self::Error> {
            // Make sure all parts of uri is accurate.
            let mut addr = uri.authority().unwrap().to_string();
            let host = uri.host().unwrap().as_str().to_string();
            let port = uri.port().unwrap().as_u16().unwrap();
            let mut auth = None;
            let mut is_proxy = false;
            let mut proxy_scheme = None;
            let mut proxy_host = None;
            let mut proxy_tls_config = None;

            if let Some(proxy) = self.config.proxies.match_proxy(uri) {
                let info = proxy.intercept.proxy_info();
                addr = proxy.via_proxy(uri).authority().unwrap().to_string();
                auth = info.basic_auth.as_ref().and_then(|v| v.to_string().ok());
                proxy_scheme = Some(info.scheme().clone());
                proxy_host = Some(info.authority().host().as_str().to_string());
                proxy_tls_config = info.tls_config().cloned();
                is_proxy = true;
            }

            match *uri.scheme().unwrap() {
                Scheme::HTTP => {
                    let tcp_stream = TcpStream::connect(addr.clone()).map_err(|e| {
                        HttpClientError::from_error(ErrorKind::Connect, e)
                    })?;
                    if is_proxy && proxy_scheme == Some(Scheme::HTTPS) {
                        let proxy_config = proxy_tls_config.unwrap_or_default();
                        let proxy_host = proxy_host.unwrap_or_else(|| addr.clone());
                        let proxy_tls = connect_tls(
                            &proxy_config,
                            proxy_host.as_str(),
                            tcp_stream,
                            addr.as_str(),
                        )?;
                        Ok(MixStream::ProxyHttps(proxy_tls))
                    } else {
                        Ok(MixStream::Http(tcp_stream))
                    }
                }
                Scheme::HTTPS => {
                    let origin_pin_host = format!("{host}:{port}");
                    let tcp_stream = TcpStream::connect(addr.as_str())
                        .map_err(|e| HttpClientError::from_error(ErrorKind::Connect, e))?;
                    if is_proxy && proxy_scheme == Some(Scheme::HTTPS) {
                        let proxy_config = proxy_tls_config.unwrap_or_default();
                        let proxy_host = proxy_host.unwrap_or_else(|| addr.clone());
                        let proxy_tls = connect_tls(
                            &proxy_config,
                            proxy_host.as_str(),
                            tcp_stream,
                            addr.as_str(),
                        )?;
                        let tunneled = tunnel(proxy_tls, host.clone(), port, auth)?;
                        let origin_tls = connect_tls(
                            &self.config.tls,
                            host.as_str(),
                            tunneled,
                            origin_pin_host.as_str(),
                        )?;
                        Ok(MixStream::HttpsOverProxy(origin_tls))
                    } else {
                        let tcp_stream = if is_proxy {
                            tunnel(tcp_stream, host.clone(), port, auth)?
                        } else {
                            tcp_stream
                        };
                        let origin_tls = connect_tls(
                            &self.config.tls,
                            host.as_str(),
                            tcp_stream,
                            origin_pin_host.as_str(),
                        )?;
                        Ok(MixStream::Https(origin_tls))
                    }
                }
            }
        }
    }

    fn connect_tls<S>(
        config: &TlsConfig,
        domain: &str,
        stream: S,
        pin_host: &str,
    ) -> Result<SslStream<S>, HttpClientError>
    where
        S: Read + Write,
    {
        let pinned_key = config.pinning_host_match(pin_host);
        let ssl = config
            .ssl_new(domain)
            .map_err(|e| HttpClientError::from_error(ErrorKind::Connect, e))?;
        let mut stream = SslStream::new_base(ssl.into_inner(), stream, pinned_key)
            .map_err(|e| HttpClientError::from_error(ErrorKind::Connect, e))?;
        stream
            .connect()
            .map_err(|e| HttpClientError::from_error(ErrorKind::Connect, e))?;
        Ok(stream)
    }

    fn tunnel<S>(
        mut conn: S,
        host: String,
        port: u16,
        auth: Option<String>,
    ) -> Result<S, HttpClientError>
    where
        S: Read + Write,
    {
        let mut req = Vec::new();

        // `unwrap()` never failed here.
        write!(
            &mut req,
            "CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n"
        )
        .unwrap();

        if let Some(value) = auth {
            write!(&mut req, "Proxy-Authorization: Basic {value}\r\n").unwrap();
        }

        write!(&mut req, "\r\n").unwrap();

        conn.write_all(&req)
            .map_err(|e| HttpClientError::from_error(ErrorKind::Connect, e))?;

        let mut buf = [0; 8192];
        let mut pos = 0;

        loop {
            let n = conn
                .read(&mut buf[pos..])
                .map_err(|e| HttpClientError::from_error(ErrorKind::Connect, e))?;

            if n == 0 {
                return Err(HttpClientError::from_str(
                    ErrorKind::Connect,
                    "Error receiving from proxy",
                ));
            }

            pos += n;
            let resp = &buf[..pos];
            if resp.starts_with(b"HTTP/1.1 200") {
                if resp.ends_with(b"\r\n\r\n") {
                    return Ok(conn);
                }
                if pos == buf.len() {
                    return Err(HttpClientError::from_str(
                        ErrorKind::Connect,
                        "proxy headers too long for tunnel",
                    ));
                }
            } else if resp.starts_with(b"HTTP/1.1 407") {
                return Err(HttpClientError::from_str(
                    ErrorKind::Connect,
                    "proxy authentication required",
                ));
            } else {
                return Err(HttpClientError::from_str(
                    ErrorKind::Connect,
                    "unsuccessful tunnel",
                ));
            }
        }
    }
}
