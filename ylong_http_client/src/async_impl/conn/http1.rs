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

use std::mem::take;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use ylong_http::body::async_impl::Body;
use ylong_http::body::{ChunkBody, TextBody};
use ylong_http::h1::{RequestEncoder, ResponseDecoder};
use ylong_http::request::uri::Scheme;
use ylong_http::response::ResponsePart;
use ylong_http::version::Version;

use super::StreamData;
use crate::async_impl::request::Message;
use crate::async_impl::{HttpBody, Request, Response};
use crate::error::HttpClientError;
use crate::runtime::{poll_fn, AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use crate::util::config::HttpVersion;
use crate::util::dispatcher::http1::Http1Conn;
use crate::util::information::ConnInfo;
use crate::util::interceptor::InterceptorContext;
use crate::util::normalizer::{header_value_contains, BodyLengthParser};
use crate::ErrorKind::BodyTransfer;

const TEMP_BUF_SIZE: usize = 16 * 1024;

#[derive(Debug, Eq, PartialEq)]
enum RequestBodyEncoding {
    None,
    Text,
    Chunk,
}

pub(crate) async fn request<S>(
    mut conn: Http1Conn<S>,
    mut message: Message,
) -> Result<Response, HttpClientError>
where
    S: AsyncRead + AsyncWrite + ConnInfo + Sync + Send + Unpin + 'static,
{
    message
        .interceptor
        .intercept_request(message.request.ref_mut())?;
    let mut buf = vec![0u8; TEMP_BUF_SIZE];

    message
        .request
        .ref_mut()
        .time_group_mut()
        .set_transfer_start(Instant::now());
    let mut guard = conn.cancel_guard();
    encode_request_part(
        message.request.ref_mut(),
        &message.interceptor,
        &mut conn,
        &mut buf,
    )
    .await?;
    encode_various_body(message.request.ref_mut(), &mut conn, &mut buf).await?;
    if let Err(e) = conn.raw_mut().flush().await {
        conn.shutdown();
        return err_from_io!(Request, e);
    }
    message
        .request
        .ref_mut()
        .time_group_mut()
        .set_request_write_end(Instant::now());
    // Decodes response part.
    let (part, pre) = {
        let mut decoder = ResponseDecoder::new();
        loop {
            let size = poll_fn(|cx| {
                if conn.speed_controller.poll_recv_pending_timeout(cx) {
                    return Poll::Ready(Err(HttpClientError::from_str(
                        BodyTransfer,
                        "Below low speed limit",
                    )));
                }
                let result =
                    read_status_line(cx, &mut conn, message.request.ref_mut(), buf.as_mut_slice())?;
                if let Poll::Ready(filled) = result {
                    conn.speed_controller.reset_recv_pending_timeout();
                    return Poll::Ready(Ok(filled));
                }
                Poll::Pending
            })
            .await?;

            if !message.interceptor.is_empty() {
                message.interceptor.intercept_output(&buf[..size])?;
            }
            match decoder.decode(&buf[..size]) {
                Ok(None) => {}
                Ok(Some((part, rem))) => break (part, rem),
                Err(e) => {
                    conn.shutdown();
                    return err_from_other!(Request, e);
                }
            }
        }
    };
    guard.normal_end();
    // if task cancel occurs, we should shutdown io
    drop(guard);

    decode_response(message, part, conn, pre)
}

fn read_status_line<S>(
    cx: &mut Context<'_>,
    conn: &mut Http1Conn<S>,
    request: &mut Request,
    buf: &mut [u8],
) -> Poll<Result<usize, HttpClientError>>
where
    S: AsyncRead + Sync + Send + Unpin + 'static,
{
    let mut read_buf = ReadBuf::new(buf);
    match Pin::new(conn.raw_mut()).poll_read(cx, &mut read_buf) {
        Poll::Ready(Ok(_)) => {
            #[cfg(feature = "ylong_base")]
            let size = read_buf.filled_len();

            #[cfg(feature = "tokio_base")]
            let size = read_buf.filled().len();

            if size == 0 {
                conn.shutdown();
                return Poll::Ready(err_from_msg!(Request, "Tcp closed"));
            }
            if request.time_group_mut().transfer_end_time().is_none() {
                request.time_group_mut().set_transfer_end(Instant::now())
            }
            Poll::Ready(Ok(size))
        }
        Poll::Ready(Err(e)) => {
            conn.shutdown();
            Poll::Ready(err_from_io!(Request, e))
        }
        Poll::Pending => Poll::Pending,
    }
}

async fn encode_various_body<S>(
    request: &mut Request,
    conn: &mut Http1Conn<S>,
    buf: &mut [u8],
) -> Result<(), HttpClientError>
where
    S: AsyncRead + AsyncWrite + Sync + Send + Unpin + 'static,
{
    match request_body_encoding(request) {
        RequestBodyEncoding::None => {}
        RequestBodyEncoding::Chunk => {
            let body = request.body_mut();
            let body = ChunkBody::from_async_reader(body);
            encode_body(conn, body, buf).await?;
        }
        RequestBodyEncoding::Text => {
            let body = request.body_mut();
            let body = TextBody::from_async_reader(body);
            encode_body(conn, body, buf).await?;
        }
    };
    Ok(())
}

fn request_body_encoding(request: &Request) -> RequestBodyEncoding {
    let transfer_encoding = request
        .part()
        .headers
        .get("Transfer-Encoding")
        .is_some_and(|v| header_value_contains(v, b"chunked"));

    if transfer_encoding {
        RequestBodyEncoding::Chunk
    } else if request.body().is_empty() {
        RequestBodyEncoding::None
    } else {
        // Content-Length does not change the HTTP/1 encoder choice: every
        // non-chunked, non-empty body is encoded as a plain text body.
        RequestBodyEncoding::Text
    }
}

async fn encode_request_part<S>(
    request: &Request,
    interceptor: &InterceptorContext,
    conn: &mut Http1Conn<S>,
    buf: &mut [u8],
) -> Result<(), HttpClientError>
where
    S: AsyncRead + AsyncWrite + ConnInfo + Sync + Send + Unpin + 'static,
{
    // Encodes and sends Request-line and Headers(non-body fields).
    let mut part = request.part().clone();
    if conn.raw_mut().is_proxy() && request.uri().scheme() == Some(&Scheme::HTTP) {
        let conn_data = conn.raw_mut().conn_data();
        if let Some(auth) = conn_data.proxy_auth() {
            let auth = format!("Basic {auth}");
            let _ = part.headers.insert("Proxy-Authorization", auth.as_bytes());
        }
    }

    let mut part_encoder = RequestEncoder::new(part);
    if conn.raw_mut().is_proxy() && request.uri().scheme() == Some(&Scheme::HTTP) {
        part_encoder.absolute_uri(true);
    }
    loop {
        match part_encoder.encode(&mut buf[..]) {
            Ok(0) => break,
            Ok(written) => {
                if !interceptor.is_empty() {
                    interceptor.intercept_input(&buf[..written])?;
                }
                // RequestEncoder writes `buf` as much as possible.
                if let Err(e) = conn.raw_mut().write_all(&buf[..written]).await {
                    conn.shutdown();
                    return err_from_io!(Request, e);
                }
            }
            Err(e) => {
                conn.shutdown();
                return err_from_other!(Request, e);
            }
        }
    }
    Ok(())
}

fn decode_response<S>(
    mut message: Message,
    part: ResponsePart,
    conn: Http1Conn<S>,
    pre: &[u8],
) -> Result<Response, HttpClientError>
where
    S: AsyncRead + AsyncWrite + ConnInfo + Sync + Send + Unpin + 'static,
{
    // The shutdown function only sets the current connection to the closed state
    // and does not release the connection immediately.
    // Instead, the connection will be completely closed
    // when the body has finished reading or when the body is released.
    match part.headers.get("Connection") {
        None => {
            if part.version == Version::HTTP1_0 {
                conn.shutdown()
            }
        }
        Some(value) => {
            if part.version == Version::HTTP1_0 {
                if !header_value_contains(value, b"keep-alive") {
                    conn.shutdown()
                }
            } else if header_value_contains(value, b"close") {
                conn.shutdown()
            }
        }
    }

    let length = match BodyLengthParser::new(message.request.ref_mut().method(), &part).parse() {
        Ok(length) => length,
        Err(e) => {
            conn.shutdown();
            return Err(e);
        }
    };

    let time_group = take(message.request.ref_mut().time_group_mut());
    let body = HttpBody::new(message.interceptor, length, Box::new(conn), pre)?;
    let mut response = Response::new(ylong_http::response::Response::from_raw_parts(part, body));
    response.set_time_group(time_group);
    Ok(response)
}

async fn encode_body<S, T>(
    conn: &mut Http1Conn<S>,
    mut body: T,
    buf: &mut [u8],
) -> Result<(), HttpClientError>
where
    T: Body,
    S: AsyncRead + AsyncWrite + Sync + Send + Unpin + 'static,
{
    // Encodes Request Body.
    let mut written = 0;
    let mut end_body = false;
    while !end_body {
        if written < buf.len() {
            let result = body.data(&mut buf[written..]).await;
            let (read, end) = read_body_result::<S, T>(conn, result)?;
            written += read;
            end_body = end;
        }
        if written == buf.len() || end_body {
            conn.speed_controller.init_min_send_if_not_start();
            conn.speed_controller.init_max_send_if_not_start();
            let mut write_size = 0;
            loop {
                let write_res = poll_fn(|cx| {
                    if conn.speed_controller.poll_send_pending_timeout(cx) {
                        return Poll::Ready(Err(HttpClientError::from_str(
                            BodyTransfer,
                            "Below low speed limit",
                        )));
                    }
                    let write_poll =
                        Pin::new(conn.raw_mut()).poll_write(cx, &buf[write_size..written]);
                    if let Poll::Ready(Ok(_)) = write_poll {
                        conn.speed_controller.reset_send_pending_timeout();
                    }
                    write_poll.map_err(|e| HttpClientError::from_error(BodyTransfer, e))
                })
                .await;
                match write_res {
                    Ok(size) => write_size += size,
                    Err(e) => {
                        conn.shutdown();
                        return Err(e);
                    }
                }
                if write_size == written {
                    break;
                }
            }
            if conn.speed_controller.need_limit_max_send_speed() {
                conn.speed_controller.max_send_speed_limit(written).await;
            }
            conn.speed_controller.min_send_speed_limit(written)?;
            written = 0;
        }
    }
    Ok(())
}

fn read_body_result<S, T>(
    conn: &mut Http1Conn<S>,
    result: Result<usize, T::Error>,
) -> Result<(usize, bool), HttpClientError>
where
    T: Body,
{
    let mut curr = 0;
    let mut end_body = false;
    match result {
        Ok(0) => end_body = true,
        Ok(size) => curr = size,
        Err(e) => {
            conn.shutdown();

            let error = e.into();
            // When using `Uploader`, here we can get `UserAborted` error.
            return if error.source().is_some() {
                Err(HttpClientError::user_aborted())
            } else {
                err_from_other!(BodyTransfer, error)
            };
        }
    }
    Ok((curr, end_body))
}

impl<S: AsyncRead + Unpin> AsyncRead for Http1Conn<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.speed_controller.is_recv_unlimited() {
            return Pin::new(self.raw_mut()).poll_read(cx, buf);
        }

        if self.speed_controller.poll_recv_pending_timeout(cx) {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                HttpClientError::from_str(BodyTransfer, "Below low speed limit"),
            )));
        }
        self.speed_controller.init_min_recv_if_not_start();
        if self
            .speed_controller
            .poll_max_recv_delay_time(cx)
            .is_pending()
        {
            return Poll::Pending;
        }
        self.speed_controller.init_max_recv_if_not_start();
        match Pin::new(self.raw_mut()).poll_read(cx, buf) {
            Poll::Ready(Ok(_)) => {
                let filled: usize = buf.filled().len();
                self.speed_controller
                    .min_recv_speed_limit(filled)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                self.speed_controller.delay_max_recv_speed_limit(filled);
                self.speed_controller.reset_recv_pending_timeout();
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S: AsyncRead + Unpin> StreamData for Http1Conn<S> {
    fn shutdown(&self) {
        Self::shutdown(self)
    }

    // HTTP1 can close the "stream" after reading the data
    fn is_stream_closable(&self) -> bool {
        true
    }

    fn http_version(&self) -> HttpVersion {
        HttpVersion::Http1
    }
}

#[cfg(test)]
mod ut_http1_conn {
    use crate::async_impl::{Body as ClientBody, Request as ClientRequest};

    use super::{request_body_encoding, RequestBodyEncoding};

    #[test]
    fn ut_request_body_encoding_skips_empty_non_chunked() {
        let request = ClientRequest::builder()
            .header("Content-Length", "0")
            .body(ClientBody::empty())
            .unwrap();
        assert_eq!(request_body_encoding(&request), RequestBodyEncoding::None);
    }

    #[test]
    fn ut_request_body_encoding_keeps_empty_chunked() {
        let request = ClientRequest::builder()
            .header("Transfer-Encoding", "gzip, chunked")
            .body(ClientBody::empty())
            .unwrap();
        assert_eq!(request_body_encoding(&request), RequestBodyEncoding::Chunk);
    }

    #[test]
    fn ut_request_body_encoding_uses_text_for_non_empty() {
        let request = ClientRequest::builder()
            .body(ClientBody::slice("request body"))
            .unwrap();
        assert_eq!(request_body_encoding(&request), RequestBodyEncoding::Text);
    }
}
