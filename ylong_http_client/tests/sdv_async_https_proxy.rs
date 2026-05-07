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

#![cfg(all(
    feature = "async",
    feature = "http1_1",
    feature = "ylong_base",
    feature = "__c_openssl"
))]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver};
use std::thread;
use std::time::Duration;

use openssl::ssl::{SslAcceptor, SslFiletype, SslMethod, SslStream, SslVerifyMode};
use ylong_http_client::async_impl::{Body, Client, RequestBuilder};
use ylong_http_client::{HttpClientError, Proxy, TlsConfig, TlsFileType};

const CERT: &str = "tests/file/cert.pem";
const KEY: &str = "tests/file/key.pem";
const ROOT_CA: &str = "tests/file/root-ca.pem";

struct HttpsProxyHandle {
    addr: String,
    done: Receiver<Result<(), String>>,
}

impl HttpsProxyHandle {
    fn finish(self) {
        self.done
            .recv()
            .expect("proxy thread dropped")
            .expect("proxy assertion failed");
    }

    fn finish_allow_error(self) {
        let _ = self.done.recv_timeout(Duration::from_secs(5));
    }
}

fn start_proxy<F>(serve: F) -> HttpsProxyHandle
where
    F: FnOnce(TcpListener) -> Result<(), String> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind proxy listener");
    let addr = listener.local_addr().expect("proxy local addr").to_string();
    let (tx, rx) = channel();
    thread::spawn(move || {
        let _ = tx.send(serve(listener));
    });
    HttpsProxyHandle { addr, done: rx }
}

fn tls_acceptor(require_client_cert: bool) -> Result<SslAcceptor, String> {
    let mut acceptor =
        SslAcceptor::mozilla_intermediate(SslMethod::tls()).map_err(|e| e.to_string())?;
    acceptor
        .set_private_key_file(KEY, SslFiletype::PEM)
        .map_err(|e| e.to_string())?;
    acceptor
        .set_certificate_chain_file(CERT)
        .map_err(|e| e.to_string())?;
    if require_client_cert {
        acceptor.set_ca_file(ROOT_CA).map_err(|e| e.to_string())?;
        acceptor.set_verify(SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT);
    }
    Ok(acceptor.build())
}

fn accept_proxy_tls(
    listener: TcpListener,
    require_client_cert: bool,
) -> Result<SslStream<TcpStream>, String> {
    let acceptor = tls_acceptor(require_client_cert)?;
    let (tcp, _) = listener.accept().map_err(|e| e.to_string())?;
    acceptor.accept(tcp).map_err(|e| e.to_string())
}

fn read_headers<S: Read>(stream: &mut S) -> Result<String, String> {
    let mut data = Vec::new();
    let mut buf = [0u8; 256];
    loop {
        let n = stream.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("connection closed before headers".to_string());
        }
        data.extend_from_slice(&buf[..n]);
        if data.windows(4).any(|w| w == b"\r\n\r\n") {
            return String::from_utf8(data).map_err(|e| e.to_string());
        }
        if data.len() > 16 * 1024 {
            return Err("headers too long".to_string());
        }
    }
}

fn assert_header(headers: &str, value: &str) -> Result<(), String> {
    let lower = headers.to_ascii_lowercase().replace(": ", ":");
    if lower.contains(&value.to_ascii_lowercase().replace(": ", ":")) {
        Ok(())
    } else {
        Err(format!("missing header `{value}` in `{headers}`"))
    }
}

fn proxy_tls_config(with_client_cert: bool) -> Result<TlsConfig, HttpClientError> {
    let mut builder = TlsConfig::builder()
        .ca_file(ROOT_CA)
        .danger_accept_invalid_hostnames(true);
    if with_client_cert {
        builder = builder
            .certificate_chain_file(CERT)
            .private_key_file(KEY, TlsFileType::PEM);
    }
    builder.build()
}

fn collect_response_body(
    mut response: ylong_http_client::async_impl::Response,
) -> Result<Vec<u8>, HttpClientError> {
    ylong_runtime::block_on(async move {
        let mut out = Vec::new();
        let mut buf = [0u8; 256];
        loop {
            let n = response.data(&mut buf).await?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        Ok(out)
    })
}

fn https_proxy_client(addr: &str, proxy_tls: TlsConfig) -> Result<Client, HttpClientError> {
    let proxy = Proxy::all(format!("https://{addr}").as_str())
        .basic_auth("username", "password")
        .proxy_tls_config(proxy_tls)
        .build()?;

    Client::builder().proxy(proxy).build()
}

#[test]
fn sdv_http_target_over_https_proxy() {
    let proxy = start_proxy(|listener| {
        let mut stream = accept_proxy_tls(listener, false)?;
        let req = read_headers(&mut stream)?;
        if !req.starts_with("GET http://example.com:80/data HTTP/1.1\r\n")
            && !req.starts_with("GET http://example.com/data HTTP/1.1\r\n")
        {
            return Err(format!("unexpected request line: {req}"));
        }
        assert_header(
            &req,
            "proxy-authorization: Basic dXNlcm5hbWU6cGFzc3dvcmQ=",
        )?;
        stream
            .write_all(b"HTTP/1.1 201 OK\r\nContent-Length: 9\r\n\r\nproxy ok!")
            .map_err(|e| e.to_string())?;
        stream.flush().map_err(|e| e.to_string())?;
        Ok(())
    });

    let client = https_proxy_client(&proxy.addr, proxy_tls_config(false).unwrap()).unwrap();
    let request = RequestBuilder::new()
        .method("GET")
        .url("http://example.com/data")
        .body(Body::empty())
        .unwrap();

    let response = match ylong_runtime::block_on(async move { client.request(request).await }) {
        Ok(response) => response,
        Err(e) => {
            proxy.finish();
            panic!("client request failed: {e:?}");
        }
    };
    assert_eq!(response.status().as_u16(), 201);
    assert_eq!(collect_response_body(response).unwrap(), b"proxy ok!");
    proxy.finish();
}

#[test]
fn sdv_https_target_over_https_proxy() {
    let proxy_server = start_proxy(|listener| {
        let mut outer = accept_proxy_tls(listener, false)?;
        let connect = read_headers(&mut outer)?;
        if !connect.starts_with("CONNECT foobar.com:443 HTTP/1.1\r\n") {
            return Err(format!("unexpected CONNECT request: {connect}"));
        }
        assert_header(
            &connect,
            "proxy-authorization: Basic dXNlcm5hbWU6cGFzc3dvcmQ=",
        )?;
        outer
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .map_err(|e| e.to_string())?;

        let acceptor = tls_acceptor(false)?;
        let mut inner = acceptor.accept(outer).map_err(|e| e.to_string())?;
        let req = read_headers(&mut inner)?;
        if !req.starts_with("GET /data HTTP/1.1\r\n") {
            return Err(format!("unexpected origin request: {req}"));
        }
        assert_header(&req, "host: foobar.com")?;
        inner
            .write_all(b"HTTP/1.1 202 OK\r\nContent-Length: 9\r\n\r\norigin ok")
            .map_err(|e| e.to_string())?;
        inner.flush().map_err(|e| e.to_string())?;
        Ok(())
    });

    let proxy_tls = proxy_tls_config(false).unwrap();
    let proxy = Proxy::all(format!("https://{}", proxy_server.addr).as_str())
        .basic_auth("username", "password")
        .proxy_tls_config(proxy_tls)
        .build()
        .unwrap();
    let client = Client::builder()
        .proxy(proxy)
        .tls_ca_file(ROOT_CA)
        .build()
        .unwrap();
    let request = RequestBuilder::new()
        .method("GET")
        .url("https://foobar.com/data")
        .body(Body::empty())
        .unwrap();

    let response = match ylong_runtime::block_on(async move { client.request(request).await }) {
        Ok(response) => response,
        Err(e) => {
            proxy_server.finish();
            panic!("client request failed: {e:?}");
        }
    };
    assert_eq!(response.status().as_u16(), 202);
    assert_eq!(collect_response_body(response).unwrap(), b"origin ok");
    proxy_server.finish();
}

#[test]
fn sdv_https_proxy_mtls_success() {
    let proxy = start_proxy(|listener| {
        let mut stream = accept_proxy_tls(listener, true)?;
        if stream.ssl().peer_certificate().is_none() {
            return Err("missing client certificate".to_string());
        }
        let req = read_headers(&mut stream)?;
        if !req.starts_with("GET http://example.com:80/mtls HTTP/1.1\r\n")
            && !req.starts_with("GET http://example.com/mtls HTTP/1.1\r\n")
        {
            return Err(format!("unexpected request line: {req}"));
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nmtls")
            .map_err(|e| e.to_string())?;
        stream.flush().map_err(|e| e.to_string())?;
        Ok(())
    });

    let client = https_proxy_client(&proxy.addr, proxy_tls_config(true).unwrap()).unwrap();
    let request = RequestBuilder::new()
        .method("GET")
        .url("http://example.com/mtls")
        .body(Body::empty())
        .unwrap();

    let response = ylong_runtime::block_on(async move { client.request(request).await }).unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(collect_response_body(response).unwrap(), b"mtls");
    proxy.finish();
}

#[test]
fn sdv_https_proxy_mtls_missing_client_cert_fails() {
    let proxy = start_proxy(|listener| {
        let _ = accept_proxy_tls(listener, true)?;
        Ok(())
    });

    let client = https_proxy_client(&proxy.addr, proxy_tls_config(false).unwrap()).unwrap();
    let request = RequestBuilder::new()
        .method("GET")
        .url("http://example.com/mtls")
        .body(Body::empty())
        .unwrap();

    let result = ylong_runtime::block_on(async move { client.request(request).await });
    assert!(result.is_err());
    proxy.finish_allow_error();
}

#[test]
fn sdv_https_proxy_untrusted_ca_fails() {
    let proxy = start_proxy(|listener| {
        let _ = accept_proxy_tls(listener, false)?;
        Ok(())
    });

    let proxy_tls = TlsConfig::builder()
        .build_in_root_certs(false)
        .danger_accept_invalid_hostnames(true)
        .build()
        .unwrap();
    let client = https_proxy_client(&proxy.addr, proxy_tls).unwrap();
    let request = RequestBuilder::new()
        .method("GET")
        .url("http://example.com/data")
        .body(Body::empty())
        .unwrap();

    let result = ylong_runtime::block_on(async move { client.request(request).await });
    assert!(result.is_err());
    proxy.finish_allow_error();
}
