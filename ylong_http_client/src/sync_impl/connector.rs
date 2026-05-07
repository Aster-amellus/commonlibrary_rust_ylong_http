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
use crate::util::pool::PoolKey;

/// `Connector` trait used by `Client`. `Connector` provides synchronous
/// connection establishment interfaces.
pub trait Connector {
    /// The connection object established by `Connector::connect`.
    type Stream: Read + Write + 'static;
    /// Possible errors during connection establishment.
    type Error: Into<Box<dyn std::error::Error + Send + Sync>>;

    /// Attempts to establish a synchronous connection.
    fn connect(&self, uri: &Uri) -> Result<Self::Stream, Self::Error>;

    /// Returns the connection pool key for `uri`.
    fn pool_key(&self, uri: &Uri) -> PoolKey {
        PoolKey::new(
            uri.scheme().unwrap().clone(),
            uri.authority().unwrap().clone(),
        )
    }
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

    fn pool_key_for(&self, uri: &Uri) -> PoolKey {
        self.config.proxies.pool_key(uri)
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
    use crate::util::pool::PoolKey;

    impl Connector for super::HttpConnector {
        type Stream = TcpStream;
        type Error = Error;

        fn pool_key(&self, uri: &Uri) -> PoolKey {
            self.pool_key_for(uri)
        }

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
    use std::net::TcpStream;

    use ylong_http::request::uri::{Scheme, Uri};

    use crate::sync_impl::proxy::{connect_tls, tunnel};
    use crate::sync_impl::{Connector, MixStream};
    use crate::util::pool::PoolKey;
    use crate::{ErrorKind, HttpClientError};

    impl Connector for super::HttpConnector {
        type Stream = MixStream<TcpStream>;
        type Error = HttpClientError;

        fn pool_key(&self, uri: &Uri) -> PoolKey {
            self.pool_key_for(uri)
        }

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
                    let tcp_stream = TcpStream::connect(addr.clone())
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
}
