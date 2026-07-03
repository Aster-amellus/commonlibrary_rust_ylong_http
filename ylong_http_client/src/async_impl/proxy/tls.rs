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

use core::pin::Pin;
use std::io::{Error, ErrorKind};

use crate::async_impl::ssl_stream::AsyncSslStream;
use crate::runtime::{AsyncRead, AsyncWrite};
use crate::{HttpClientError, TlsConfig};

pub(crate) async fn connect_tls<S>(
    config: TlsConfig,
    domain: &str,
    stream: S,
    pin_host: &str,
) -> Result<AsyncSslStream<S>, HttpClientError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let pinned_key = config.pinning_host_match(pin_host);
    let mut stream = config
        .ssl_new(domain)
        .and_then(|ssl| AsyncSslStream::new(ssl.into_inner(), stream, pinned_key))
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
