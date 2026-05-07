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

//! This example shows how to configure an HTTPS proxy with a dedicated TLS
//! configuration.

use ylong_http_client::async_impl::{Body, ClientBuilder, Downloader, Request};
use ylong_http_client::{HttpClientError, Proxy, TlsConfig, TlsFileType};

#[tokio::main]
async fn main() -> Result<(), HttpClientError> {
    let proxy_tls = TlsConfig::builder()
        .ca_file("proxy-ca.pem")
        .certificate_chain_file("client-chain.pem")
        .private_key_file("client-key.pem", TlsFileType::PEM)
        .cipher_suite("TLS_AES_256_GCM_SHA384")
        .build()?;

    let proxy = Proxy::all("https://proxy.example.com:8443")
        .basic_auth("username", "password")
        .proxy_tls_config(proxy_tls)
        .build()?;

    let client = ClientBuilder::new().proxy(proxy).build()?;
    let request = Request::builder()
        .url("https://www.example.com")
        .body(Body::empty())?;

    let response = client.request(request).await?;
    let _ = Downloader::console(response).download().await;
    Ok(())
}
