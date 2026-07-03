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

//! This is an asynchronous HTTPS proxy client example.

use ylong_http_client::async_impl::{Body, ClientBuilder, Downloader, Request};
use ylong_http_client::{HttpClientError, Proxy, TlsConfig, TlsFileType, TlsVersion};

#[tokio::main]
async fn main() -> Result<(), HttpClientError> {
    let proxy_tls = TlsConfig::builder()
        .ca_file("certs/proxy-ca.pem")
        .certificate_chain_file("certs/proxy-client-chain.pem")
        .private_key_file("certs/proxy-client-key.pem", TlsFileType::PEM)
        .min_proto_version(TlsVersion::TLS_1_2)
        .cipher_suite("TLS_AES_128_GCM_SHA256")
        .build()?;

    let proxy = Proxy::all("https://proxy.example.com:8443")
        .basic_auth("username", "password")
        .proxy_tls_config(proxy_tls)
        .build()?;

    let client = ClientBuilder::new()
        .proxy(proxy)
        .tls_ca_file("certs/origin-ca.pem")
        .build()?;

    let request = Request::builder()
        .url("https://www.example.com/")
        .body(Body::empty())?;

    let response = client.request(request).await?;
    Downloader::console(response).download().await
}
