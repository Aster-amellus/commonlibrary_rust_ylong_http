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
use crate::async_impl::proxy::{connect_tls, ProxyRoute};
use crate::runtime::TcpStream;
use crate::{HttpClientError, TimeGroup};

impl ProxyRoute {
    pub(crate) async fn into_http_stream(
        self,
        tcp: TcpStream,
        next_hop: &str,
        time_group: &mut TimeGroup,
    ) -> Result<MixStream, HttpClientError> {
        self.ensure_supported()?;

        match self {
            Self::Direct | Self::Http(_) => Ok(MixStream::Http(tcp)),
            Self::Https(endpoint) => {
                let config = endpoint.tls_config().cloned().unwrap_or_default();
                time_group.set_tls_start(Instant::now());
                let stream = connect_tls(config, endpoint.host(), tcp, next_hop).await?;
                time_group.set_tls_end(Instant::now());
                Ok(MixStream::ProxyHttps(stream))
            }
        }
    }
}
