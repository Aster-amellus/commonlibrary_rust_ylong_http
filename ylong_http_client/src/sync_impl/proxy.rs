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

//! Synchronous proxy transport helpers.

use std::io::{Read, Write};

use crate::util::c_openssl::ssl::SslStream;
use crate::{ErrorKind, HttpClientError, TlsConfig};

pub(crate) fn connect_tls<S>(
    config: &TlsConfig,
    domain: &str,
    stream: S,
    pin_host: &str,
) -> Result<SslStream<S>, HttpClientError>
where
    S: Read + Write,
{
    connect_tls_with_read_ahead(config, domain, stream, pin_host, false)
}

pub(crate) fn connect_tls_with_read_ahead<S>(
    config: &TlsConfig,
    domain: &str,
    stream: S,
    pin_host: &str,
    read_ahead: bool,
) -> Result<SslStream<S>, HttpClientError>
where
    S: Read + Write,
{
    let pinned_key = config.pinning_host_match(pin_host);
    let mut ssl = config
        .ssl_new(domain)
        .map(|ssl| ssl.into_inner())
        .map_err(|e| HttpClientError::from_error(ErrorKind::Connect, e))?;
    #[cfg(feature = "__c_openssl")]
    if read_ahead {
        ssl.set_read_ahead(true);
        ssl.set_default_read_buffer_len(256 * 1024);
    }
    let mut stream = SslStream::new_base(ssl, stream, pinned_key)
        .map_err(|e| HttpClientError::from_error(ErrorKind::Connect, e))?;
    stream
        .connect()
        .map_err(|e| HttpClientError::from_error(ErrorKind::Connect, e))?;
    Ok(stream)
}

pub(crate) fn tunnel<S>(
    mut conn: S,
    host: String,
    port: u16,
    auth: Option<String>,
) -> Result<S, HttpClientError>
where
    S: Read + Write,
{
    let mut req = Vec::new();

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
        if resp.starts_with(b"HTTP/1.1 200") || resp.starts_with(b"HTTP/1.0 200") {
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
