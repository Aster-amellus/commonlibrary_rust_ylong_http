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

use std::error;
use std::fmt::{Debug, Display, Formatter};
use std::io::{Error, ErrorKind, Write};

use crate::runtime::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

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
    conn.flush().await?;

    let mut buf = [0; 8192];
    let mut pos = 0;

    loop {
        let n = conn.read(&mut buf[pos..]).await?;

        if n == 0 {
            return Err(other_io_error(CreateTunnelErr::Unsuccessful));
        }

        pos += n;
        let resp = &buf[..pos];
        match tunnel_response_status(resp) {
            TunnelResponseStatus::Incomplete => {
                if pos == buf.len() {
                    return Err(other_io_error(CreateTunnelErr::ProxyHeadersTooLong));
                }
            }
            TunnelResponseStatus::Successful => {
                if headers_complete(resp) {
                    return Ok(conn);
                }
                if pos == buf.len() {
                    return Err(other_io_error(CreateTunnelErr::ProxyHeadersTooLong));
                }
            }
            TunnelResponseStatus::AuthenticationRequired => {
                return Err(other_io_error(CreateTunnelErr::ProxyAuthenticationRequired));
            }
            TunnelResponseStatus::Unsuccessful => {
                return Err(other_io_error(CreateTunnelErr::Unsuccessful));
            }
        }
    }
}

enum TunnelResponseStatus {
    Incomplete,
    Successful,
    AuthenticationRequired,
    Unsuccessful,
}

fn tunnel_response_status(resp: &[u8]) -> TunnelResponseStatus {
    let Some(status_line_end) = resp.windows(2).position(|line| line == b"\r\n") else {
        return TunnelResponseStatus::Incomplete;
    };

    let status_line = &resp[..status_line_end];
    let Some(code) = http_status_code(status_line) else {
        return TunnelResponseStatus::Unsuccessful;
    };

    match code {
        b"200" => TunnelResponseStatus::Successful,
        b"407" => TunnelResponseStatus::AuthenticationRequired,
        _ => TunnelResponseStatus::Unsuccessful,
    }
}

fn http_status_code(status_line: &[u8]) -> Option<&[u8]> {
    let mut parts = status_line.splitn(3, |byte| *byte == b' ');
    let version = parts.next()?;
    if version != b"HTTP/1.1" && version != b"HTTP/1.0" {
        return None;
    }

    let code = parts.next()?;
    if code.len() == 3 && code.iter().all(|byte| byte.is_ascii_digit()) {
        Some(code)
    } else {
        None
    }
}

fn headers_complete(resp: &[u8]) -> bool {
    resp.windows(4).any(|part| part == b"\r\n\r\n")
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

#[cfg(test)]
mod ut_tunnel_error_debug {
    use super::CreateTunnelErr;

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

#[cfg(all(test, feature = "ylong_base"))]
mod ut_tunnel {
    use core::pin::Pin;
    use std::io;
    use std::task::{Context, Poll};

    #[cfg(feature = "__c_openssl")]
    use openssl as _;

    use super::{other_io_error, tunnel, CreateTunnelErr};
    use crate::runtime::{AsyncRead, AsyncWrite, ReadBuf};

    #[derive(Default)]
    struct MockStream {
        response: Vec<u8>,
        written: Vec<u8>,
        flushed: bool,
        read_pos: usize,
        read_limit: usize,
    }

    impl MockStream {
        fn new(response: &[u8]) -> Self {
            Self {
                response: response.to_vec(),
                read_limit: usize::MAX,
                ..Self::default()
            }
        }

        fn with_read_limit(mut self, read_limit: usize) -> Self {
            self.read_limit = read_limit;
            self
        }
    }

    impl AsyncRead for MockStream {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let read = self
                .response
                .len()
                .saturating_sub(self.read_pos)
                .min(buf.remaining())
                .min(self.read_limit);
            let end = self.read_pos + read;
            let data = self.response[self.read_pos..end].to_vec();
            buf.append(&data);
            self.read_pos = end;
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for MockStream {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.written.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.flushed = true;
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    /// UT test cases for `tunnel`.
    ///
    /// # Brief
    /// 1. Creates a stream with unsuccessful proxy responses.
    /// 2. Sends a `Request` by `tunnel`.
    /// 3. Checks if the result is as expected.
    #[test]
    fn ut_ssl_tunnel_error() {
        ylong_runtime::block_on(async {
            let res = tunnel(
                MockStream::new(b""),
                "ylong_http.com",
                443,
                Some(String::from("base64 bytes")),
            )
            .await;
            assert_eq!(
                format!("{:?}", res.err()),
                format!("{:?}", Some(other_io_error(CreateTunnelErr::Unsuccessful)))
            );

            let res = tunnel(
                MockStream::new(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n"),
                "ylong_http.com",
                443,
                Some(String::from("base64 bytes")),
            )
            .await;
            assert_eq!(
                format!("{:?}", res.err()),
                format!(
                    "{:?}",
                    Some(other_io_error(CreateTunnelErr::ProxyAuthenticationRequired))
                )
            );

            let res = tunnel(
                MockStream::new(b"HTTP/1.1 402 Payment Required\r\n\r\n"),
                "ylong_http.com",
                443,
                Some(String::from("base64 bytes")),
            )
            .await;
            assert_eq!(
                format!("{:?}", res.err()),
                format!("{:?}", Some(other_io_error(CreateTunnelErr::Unsuccessful)))
            );
        });
    }

    /// UT test cases for `tunnel`.
    ///
    /// # Brief
    /// 1. Creates a stream with a successful proxy response.
    /// 2. Sends a `Request` by `tunnel`.
    /// 3. Checks if the result is as expected.
    #[test]
    fn ut_ssl_tunnel_connect() {
        ylong_runtime::block_on(async {
            let res = tunnel(
                MockStream::new(b"HTTP/1.1 200 Connection Established\r\n\r\n"),
                "ylong_http.com",
                443,
                Some(String::from("base64 bytes")),
            )
            .await;
            assert!(res.is_ok());
        });
    }

    /// UT test cases for split proxy response status line.
    ///
    /// # Brief
    /// 1. Creates a stream that returns a successful response in small reads.
    /// 2. Sends a `Request` by `tunnel`.
    /// 3. Checks that a valid split response is accepted.
    #[test]
    fn ut_ssl_tunnel_accepts_split_status_line() {
        ylong_runtime::block_on(async {
            let res = tunnel(
                MockStream::new(b"HTTP/1.1 200 Connection Established\r\n\r\n").with_read_limit(6),
                "ylong_http.com",
                443,
                Some(String::from("base64 bytes")),
            )
            .await;
            assert!(res.is_ok(), "tunnel failed: {:?}", res.as_ref().err());
        });
    }

    /// UT test cases for response beyond size of `tunnel`.
    ///
    /// # Brief
    /// 1. Creates a stream with a too-long proxy response.
    /// 2. Sends a `Request` by `tunnel`.
    /// 3. Checks if the result is as expected.
    #[test]
    fn ut_ssl_tunnel_resp_beyond_size() {
        ylong_runtime::block_on(async {
            let mut response = b"HTTP/1.1 200 Connection Established\r\n".to_vec();
            response.resize(8193, b'b');
            let res = tunnel(
                MockStream::new(&response),
                "ylong_http.com",
                443,
                Some(String::from("base64 bytes")),
            )
            .await;
            assert_eq!(
                format!("{:?}", res.err()),
                format!(
                    "{:?}",
                    Some(other_io_error(CreateTunnelErr::ProxyHeadersTooLong))
                )
            );
        });
    }
}

#[cfg(all(test, feature = "ylong_base"))]
mod ut_tunnel_flush {
    use core::pin::Pin;
    use std::io;
    use std::task::{Context, Poll};

    use super::tunnel;
    use crate::runtime::{AsyncRead, AsyncWrite, ReadBuf};

    const RESPONSE: &[u8] = b"HTTP/1.1 200 Connection Established\r\n\r\n";

    #[derive(Default)]
    struct FlushGatedStream {
        written: Vec<u8>,
        flushed: bool,
        read_pos: usize,
    }

    impl AsyncRead for FlushGatedStream {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if !self.flushed {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::Other,
                    "read before flush",
                )));
            }

            let read = RESPONSE
                .len()
                .saturating_sub(self.read_pos)
                .min(buf.remaining());
            let end = self.read_pos + read;
            buf.append(&RESPONSE[self.read_pos..end]);
            self.read_pos = end;
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for FlushGatedStream {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.written.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.flushed = true;
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[test]
    fn ut_async_tunnel_flushes_connect_request() {
        ylong_runtime::block_on(async {
            let res = tunnel(FlushGatedStream::default(), "example.com", 443, None).await;
            assert!(res.is_ok(), "tunnel failed: {:?}", res.as_ref().err());
            let stream = match res {
                Ok(stream) => stream,
                Err(_) => return,
            };
            assert!(stream.flushed);
            assert_eq!(
                stream.written,
                b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n"
            );
        });
    }
}
