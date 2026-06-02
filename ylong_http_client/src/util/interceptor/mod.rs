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

//! Http network interceptor.

#[cfg(feature = "async")]
use ylong_http::response::Response as HttpResp;

use std::sync::Arc;

#[cfg(feature = "async")]
use crate::async_impl::{HttpBody, Request, Response};
use crate::{ConnDetail, HttpClientError};

pub(crate) type Interceptors = dyn Interceptor + Sync + Send + 'static;

/// Optional interceptor wrapper.
///
/// The default client has no installed interceptor, so hot paths can skip
/// no-op dynamic dispatch. User-provided interceptors keep the same behavior.
#[derive(Clone, Default)]
pub(crate) struct InterceptorContext {
    inner: Option<Arc<Interceptors>>,
}

impl InterceptorContext {
    pub(crate) fn none() -> Self {
        Self { inner: None }
    }

    pub(crate) fn new<T>(interceptor: T) -> Self
    where
        T: Interceptor + Sync + Send + 'static,
    {
        Self {
            inner: Some(Arc::new(interceptor)),
        }
    }

    pub(crate) fn intercept_connection(&self, info: ConnDetail) -> Result<(), HttpClientError> {
        if let Some(interceptor) = &self.inner {
            interceptor.intercept_connection(info)?;
        }
        Ok(())
    }

    pub(crate) fn intercept_input(&self, bytes: &[u8]) -> Result<(), HttpClientError> {
        if let Some(interceptor) = &self.inner {
            interceptor.intercept_input(bytes)?;
        }
        Ok(())
    }

    pub(crate) fn intercept_output(&self, bytes: &[u8]) -> Result<(), HttpClientError> {
        if let Some(interceptor) = &self.inner {
            interceptor.intercept_output(bytes)?;
        }
        Ok(())
    }

    #[cfg(feature = "async")]
    pub(crate) fn intercept_request(&self, request: &Request) -> Result<(), HttpClientError> {
        if let Some(interceptor) = &self.inner {
            interceptor.intercept_request(request)?;
        }
        Ok(())
    }

    #[cfg(feature = "async")]
    pub(crate) fn intercept_response(&self, response: &Response) -> Result<(), HttpClientError> {
        if let Some(interceptor) = &self.inner {
            interceptor.intercept_response(response)?;
        }
        Ok(())
    }

    pub(crate) fn intercept_retry(&self, error: &HttpClientError) -> Result<(), HttpClientError> {
        if let Some(interceptor) = &self.inner {
            interceptor.intercept_retry(error)?;
        }
        Ok(())
    }

    #[cfg(feature = "async")]
    pub(crate) fn intercept_redirect_request(
        &self,
        request: &Request,
    ) -> Result<(), HttpClientError> {
        if let Some(interceptor) = &self.inner {
            interceptor.intercept_redirect_request(request)?;
        }
        Ok(())
    }

    #[cfg(feature = "async")]
    pub(crate) fn intercept_redirect_response(
        &self,
        response: &HttpResp<HttpBody>,
    ) -> Result<(), HttpClientError> {
        if let Some(interceptor) = &self.inner {
            interceptor.intercept_redirect_response(response)?;
        }
        Ok(())
    }
}

/// Transport layer protocol type.
#[derive(Clone)]
pub enum ConnProtocol {
    /// Tcp protocol.
    Tcp,
    /// Udp Protocol.
    Udp,
    /// Quic Protocol
    Quic,
}

/// Network interceptor.
///
/// Provides intercepting behavior at various stages of http message passing.
pub trait Interceptor {
    /// Intercepts the created transport layer protocol.
    // TODO add cache and response interceptor.
    // Is it necessary to add a response interceptor?
    // Does the input and output interceptor need to be added to http2 or http3
    // encoded packets?
    fn intercept_connection(&self, _info: ConnDetail) -> Result<(), HttpClientError> {
        Ok(())
    }

    /// Intercepts the input of transport layer io.
    fn intercept_input(&self, _bytes: &[u8]) -> Result<(), HttpClientError> {
        Ok(())
    }

    /// Intercepts the output of transport layer io.
    fn intercept_output(&self, _bytes: &[u8]) -> Result<(), HttpClientError> {
        Ok(())
    }

    /// Intercepts the Request that is eventually transmitted to the peer end.
    #[cfg(feature = "async")]
    fn intercept_request(&self, _request: &Request) -> Result<(), HttpClientError> {
        Ok(())
    }

    /// Intercepts the response that is eventually returned.
    #[cfg(feature = "async")]
    fn intercept_response(&self, _response: &Response) -> Result<(), HttpClientError> {
        Ok(())
    }

    /// Intercepts the error cause of the retry.
    fn intercept_retry(&self, _error: &HttpClientError) -> Result<(), HttpClientError> {
        Ok(())
    }

    /// Intercepts the redirect request.
    #[cfg(feature = "async")]
    fn intercept_redirect_request(&self, _request: &Request) -> Result<(), HttpClientError> {
        Ok(())
    }

    /// Intercepts the response returned by the redirect
    #[cfg(feature = "async")]
    fn intercept_redirect_response(
        &self,
        _response: &HttpResp<HttpBody>,
    ) -> Result<(), HttpClientError> {
        Ok(())
    }
}
