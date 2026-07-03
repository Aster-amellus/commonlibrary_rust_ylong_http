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

use std::time::Instant;

use crate::async_impl::mix::MixStream;
use crate::async_impl::proxy::{connect_tls, tunnel, ProxyRoute};
use crate::runtime::TcpStream;
use crate::{HttpClientError, TimeGroup, TlsConfig};

impl ProxyRoute {
    pub(crate) async fn into_https_stream(
        self,
        tcp: TcpStream,
        next_hop: &str,
        origin_host: &str,
        origin_port: u16,
        origin_tls: TlsConfig,
        time_group: &mut TimeGroup,
    ) -> Result<MixStream, HttpClientError> {
        self.ensure_supported()?;

        let origin_pin_host = format!("{origin_host}:{origin_port}");
        time_group.set_tls_start(Instant::now());

        let stream = match self {
            Self::Direct => {
                let origin =
                    connect_tls(origin_tls, origin_host, tcp, origin_pin_host.as_str()).await?;
                MixStream::Https(origin)
            }
            Self::Http(endpoint) => {
                let tunneled = tunnel(
                    tcp,
                    origin_host,
                    origin_port,
                    endpoint.auth().map(String::from),
                )
                .await
                .map_err(|e| HttpClientError::from_io_error(crate::ErrorKind::Connect, e))?;
                let origin =
                    connect_tls(origin_tls, origin_host, tunneled, origin_pin_host.as_str())
                        .await?;
                MixStream::Https(origin)
            }
            Self::Https(endpoint) => {
                let proxy_config = endpoint.tls_config().cloned().unwrap_or_default();
                let proxy_tls = connect_tls(proxy_config, endpoint.host(), tcp, next_hop).await?;
                let tunneled = tunnel(
                    proxy_tls,
                    origin_host,
                    origin_port,
                    endpoint.auth().map(String::from),
                )
                .await
                .map_err(|e| HttpClientError::from_io_error(crate::ErrorKind::Connect, e))?;
                let origin =
                    connect_tls(origin_tls, origin_host, tunneled, origin_pin_host.as_str())
                        .await?;
                MixStream::HttpsOverProxy(origin)
            }
        };

        time_group.set_tls_end(Instant::now());
        Ok(stream)
    }
}
