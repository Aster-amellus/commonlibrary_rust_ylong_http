// Copyright (c) 2026 Huawei Device Co., Ltd.
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

//! Async proxy transport helpers.

use core::pin::Pin;
use std::error;
use std::fmt::{Debug, Display, Formatter};
use std::io::{Error, ErrorKind, Write};

use crate::async_impl::ssl_stream::AsyncSslStream;
use crate::runtime::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use crate::{HttpClientError, TlsConfig};

pub(crate) const DEFAULT_READ_AHEAD_BUFFER: usize = 256 * 1024;
pub(crate) const CONNECT_PROXY_READ_AHEAD_BUFFER: usize = 64 * 1024;
pub(crate) const ORIGIN_TLS_READ_AHEAD_BUFFER: usize = 8 * 1024;

pub(crate) async fn connect_tls<S>(
    config: TlsConfig,
    domain: &str,
    stream: S,
    pin_host: &str,
) -> Result<AsyncSslStream<S>, HttpClientError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    connect_tls_with_read_ahead(config, domain, stream, pin_host, false).await
}

pub(crate) async fn connect_tls_with_read_ahead_buffer<S>(
    config: TlsConfig,
    domain: &str,
    stream: S,
    pin_host: &str,
    read_buffer_len: usize,
) -> Result<AsyncSslStream<S>, HttpClientError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    connect_tls_inner(config, domain, stream, pin_host, Some(read_buffer_len)).await
}

pub(crate) async fn connect_tls_with_read_ahead<S>(
    config: TlsConfig,
    domain: &str,
    stream: S,
    pin_host: &str,
    read_ahead: bool,
) -> Result<AsyncSslStream<S>, HttpClientError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let read_buffer_len = read_ahead.then_some(DEFAULT_READ_AHEAD_BUFFER);
    connect_tls_inner(config, domain, stream, pin_host, read_buffer_len).await
}

async fn connect_tls_inner<S>(
    config: TlsConfig,
    domain: &str,
    stream: S,
    pin_host: &str,
    read_buffer_len: Option<usize>,
) -> Result<AsyncSslStream<S>, HttpClientError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let pinned_key = config.pinning_host_match(pin_host);
    let mut stream = config
        .ssl_new(domain)
        .and_then(|ssl| {
            let mut ssl = ssl.into_inner();
            #[cfg(feature = "__c_openssl")]
            if let Some(len) = read_buffer_len {
                ssl.set_read_ahead(true);
                ssl.set_default_read_buffer_len(len);
            }
            AsyncSslStream::new(ssl, stream, pinned_key)
        })
        .map_err(|e| {
            HttpClientError::from_tls_error(
                crate::ErrorKind::Connect,
                Error::new(ErrorKind::Other, e),
            )
        })?;

    Pin::new(&mut stream).connect().await.map_err(|e| {
        HttpClientError::from_tls_error(crate::ErrorKind::Connect, Error::new(ErrorKind::Other, e))
    })?;
    Ok(stream)
}

pub(crate) async fn tunnel<S>(
    mut conn: S,
    host: &str,
    port: u16,
    auth: Option<String>,
) -> Result<S, Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut req = Vec::new();

    write!(
        &mut req,
        "CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n"
    )?;

    if let Some(value) = auth {
        write!(&mut req, "Proxy-Authorization: Basic {value}\r\n")?;
    }

    write!(&mut req, "\r\n")?;

    conn.write_all(&req).await?;

    let mut buf = [0; 8192];
    let mut pos = 0;

    loop {
        let n = conn.read(&mut buf[pos..]).await?;

        if n == 0 {
            return Err(other_io_error(CreateTunnelErr::Unsuccessful));
        }

        pos += n;
        let resp = &buf[..pos];
        if resp.starts_with(b"HTTP/1.1 200") || resp.starts_with(b"HTTP/1.0 200") {
            if resp.ends_with(b"\r\n\r\n") {
                return Ok(conn);
            }
            if pos == buf.len() {
                return Err(other_io_error(CreateTunnelErr::ProxyHeadersTooLong));
            }
        } else if resp.starts_with(b"HTTP/1.1 407") {
            return Err(other_io_error(CreateTunnelErr::ProxyAuthenticationRequired));
        } else {
            return Err(other_io_error(CreateTunnelErr::Unsuccessful));
        }
    }
}

pub(crate) fn other_io_error(err: CreateTunnelErr) -> Error {
    Error::new(ErrorKind::Other, err)
}

pub(crate) enum CreateTunnelErr {
    ProxyHeadersTooLong,
    ProxyAuthenticationRequired,
    Unsuccessful,
}

impl Debug for CreateTunnelErr {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProxyHeadersTooLong => f.write_str("Proxy headers too long for tunnel"),
            Self::ProxyAuthenticationRequired => f.write_str("Proxy authentication required"),
            Self::Unsuccessful => f.write_str("Unsuccessful tunnel"),
        }
    }
}

impl Display for CreateTunnelErr {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(self, f)
    }
}

impl error::Error for CreateTunnelErr {}

#[cfg(all(test, feature = "__tls"))]
mod ut_tunnel_error_debug {
    use crate::async_impl::proxy::CreateTunnelErr;

    /// UT test cases for debug of`CreateTunnelErr`.
    ///
    /// # Brief
    /// 1. Checks `CreateTunnelErr` debug by calling `CreateTunnelErr::fmt`.
    /// 2. Checks if the result is as expected.
    #[test]
    fn ut_tunnel_error_debug_assert() {
        assert_eq!(
            format!("{:?}", CreateTunnelErr::ProxyHeadersTooLong),
            "Proxy headers too long for tunnel"
        );
        assert_eq!(
            format!("{:?}", CreateTunnelErr::ProxyAuthenticationRequired),
            "Proxy authentication required"
        );
        assert_eq!(
            format!("{:?}", CreateTunnelErr::Unsuccessful),
            "Unsuccessful tunnel"
        );
        assert_eq!(
            format!("{}", CreateTunnelErr::ProxyHeadersTooLong),
            "Proxy headers too long for tunnel"
        );
        assert_eq!(
            format!("{}", CreateTunnelErr::ProxyAuthenticationRequired),
            "Proxy authentication required"
        );
        assert_eq!(
            format!("{}", CreateTunnelErr::Unsuccessful),
            "Unsuccessful tunnel"
        );
    }
}
