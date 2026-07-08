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

use ylong_http::request::uri::{Authority, Scheme, Uri};

use crate::util::proxy::{Proxies, Proxy};
use crate::HttpClientError;
#[cfg(feature = "__tls")]
use crate::TlsConfig;

#[cfg(not(feature = "__tls"))]
const HTTPS_PROXY_REQUIRES_TLS: &str = "HTTPS proxy requires TLS feature";

#[allow(dead_code)]
pub(crate) enum ProxyRoute {
    Direct,
    Http(ProxyEndpoint),
    #[cfg(feature = "__tls")]
    Https(ProxyEndpoint),
    #[cfg(not(feature = "__tls"))]
    HttpsUnsupported(ProxyEndpoint),
}

#[allow(dead_code)]
impl ProxyRoute {
    pub(crate) fn resolve(proxies: &Proxies, target: &Uri) -> Self {
        proxies
            .match_proxy(target)
            .map(Self::from_proxy)
            .unwrap_or(Self::Direct)
    }

    pub(crate) fn ensure_supported(&self) -> Result<(), HttpClientError> {
        match self {
            #[cfg(not(feature = "__tls"))]
            Self::HttpsUnsupported(_) => Err(HttpClientError::from_str(
                crate::ErrorKind::Connect,
                HTTPS_PROXY_REQUIRES_TLS,
            )),
            _ => Ok(()),
        }
    }

    pub(crate) fn next_hop_authority(&self, target: &Uri) -> Authority {
        <Self as ProxyTransport>::next_hop_authority(self, target)
    }

    pub(crate) fn pool_key(&self) -> Option<(u64, Scheme, Authority)> {
        <Self as ProxyTransport>::pool_key(self)
    }

    pub(crate) fn proxy_auth(&self) -> Option<String> {
        self.proxy_auth_ref().map(String::from)
    }

    pub(crate) fn is_proxied(&self) -> bool {
        <Self as ProxyTransport>::is_proxied(self)
    }

    fn from_proxy(proxy: &Proxy) -> Self {
        let endpoint = ProxyEndpoint::from_proxy(proxy);

        if endpoint.scheme == Scheme::HTTPS {
            #[cfg(feature = "__tls")]
            {
                Self::Https(endpoint)
            }
            #[cfg(not(feature = "__tls"))]
            {
                Self::HttpsUnsupported(endpoint)
            }
        } else {
            Self::Http(endpoint)
        }
    }

    fn endpoint(&self) -> Option<&ProxyEndpoint> {
        match self {
            Self::Direct => None,
            Self::Http(endpoint) => Some(endpoint),
            #[cfg(feature = "__tls")]
            Self::Https(endpoint) => Some(endpoint),
            #[cfg(not(feature = "__tls"))]
            Self::HttpsUnsupported(endpoint) => Some(endpoint),
        }
    }
}

#[allow(dead_code)]
pub(crate) struct ProxyEndpoint {
    pool_key_id: u64,
    scheme: Scheme,
    authority: Authority,
    basic_auth: Option<String>,
    #[cfg(feature = "__tls")]
    tls_config: Option<TlsConfig>,
}

#[allow(dead_code)]
impl ProxyEndpoint {
    pub(crate) fn from_proxy(proxy: &Proxy) -> Self {
        let info = proxy.intercept.proxy_info();

        Self {
            pool_key_id: info.pool_key_id(),
            scheme: info.scheme().clone(),
            authority: info.authority().clone(),
            basic_auth: info
                .basic_auth
                .as_ref()
                .and_then(|auth| auth.to_string().ok()),
            #[cfg(feature = "__tls")]
            tls_config: info.tls_config.clone(),
        }
    }

    pub(crate) fn authority(&self) -> &Authority {
        &self.authority
    }

    pub(crate) fn host(&self) -> &str {
        self.authority.host().as_str()
    }

    pub(crate) fn auth(&self) -> Option<&str> {
        self.basic_auth.as_deref()
    }

    #[cfg(feature = "__tls")]
    pub(crate) fn tls_config(&self) -> Option<&TlsConfig> {
        self.tls_config.as_ref()
    }

    fn pool_key(&self) -> (u64, Scheme, Authority) {
        (
            self.pool_key_id,
            self.scheme.clone(),
            self.authority.clone(),
        )
    }
}

#[allow(dead_code)]
pub(crate) trait ProxyTransport {
    fn next_hop_authority(&self, target: &Uri) -> Authority;

    fn pool_key(&self) -> Option<(u64, Scheme, Authority)>;

    fn proxy_auth_ref(&self) -> Option<&str>;

    fn is_proxied(&self) -> bool;
}

impl ProxyTransport for ProxyRoute {
    fn next_hop_authority(&self, target: &Uri) -> Authority {
        self.endpoint()
            .map(|endpoint| endpoint.authority().clone())
            .or_else(|| target.authority().cloned())
            .unwrap_or_default()
    }

    fn pool_key(&self) -> Option<(u64, Scheme, Authority)> {
        self.endpoint().map(ProxyEndpoint::pool_key)
    }

    fn proxy_auth_ref(&self) -> Option<&str> {
        self.endpoint().and_then(ProxyEndpoint::auth)
    }

    fn is_proxied(&self) -> bool {
        self.endpoint().is_some()
    }
}

#[cfg(test)]
mod ut_proxy_route {
    use ylong_http::request::uri::Uri;

    use super::{ProxyRoute, ProxyTransport};
    use crate::util::proxy::{Proxies, Proxy};

    #[test]
    fn ut_proxy_route_direct() {
        let uri = Uri::from_bytes(b"http://origin.example.com/data").unwrap();
        let route = ProxyRoute::resolve(&Proxies::default(), &uri);
        let _transport: &dyn ProxyTransport = &route;

        assert!(!route.is_proxied());
        assert!(route.pool_key().is_none());
        assert_eq!(
            route.next_hop_authority(&uri).to_string(),
            "origin.example.com"
        );
        assert_eq!(route.proxy_auth(), None);
        route.ensure_supported().unwrap();
    }

    #[test]
    fn ut_proxy_route_http_proxy() {
        let uri = Uri::from_bytes(b"http://origin.example.com/data").unwrap();
        let mut proxies = Proxies::default();
        proxies.add_proxy(Proxy::all("http://proxy.example.com:8080").unwrap());

        let route = ProxyRoute::resolve(&proxies, &uri);
        let (_, scheme, authority) = route.pool_key().unwrap();

        assert!(route.is_proxied());
        assert_eq!(scheme.as_str(), "http");
        assert_eq!(authority.to_string(), "proxy.example.com:8080");
        assert_eq!(
            route.next_hop_authority(&uri).to_string(),
            "proxy.example.com:8080"
        );
        route.ensure_supported().unwrap();
    }

    #[test]
    fn ut_proxy_route_no_proxy_uses_direct_route() {
        let uri = Uri::from_bytes(b"http://origin.example.com/data").unwrap();
        let mut proxy = Proxy::all("http://proxy.example.com:8080").unwrap();
        proxy.no_proxy("origin.example.com");

        let mut proxies = Proxies::default();
        proxies.add_proxy(proxy);

        let route = ProxyRoute::resolve(&proxies, &uri);
        assert!(!route.is_proxied());
        assert!(route.pool_key().is_none());
        assert_eq!(
            route.next_hop_authority(&uri).to_string(),
            "origin.example.com"
        );
    }

    #[test]
    fn ut_proxy_route_https_proxy_no_proxy_uses_direct_route() {
        let uri = Uri::from_bytes(b"https://origin.example.com/data").unwrap();
        let mut proxy = Proxy::all("https://proxy.example.com:8443").unwrap();
        proxy.no_proxy("origin.example.com");

        let mut proxies = Proxies::default();
        proxies.add_proxy(proxy);

        let route = ProxyRoute::resolve(&proxies, &uri);
        assert!(!route.is_proxied());
        assert!(route.pool_key().is_none());
        assert_eq!(
            route.next_hop_authority(&uri).to_string(),
            "origin.example.com"
        );
        route.ensure_supported().unwrap();
    }

    #[test]
    fn ut_proxy_route_basic_auth() {
        let uri = Uri::from_bytes(b"https://origin.example.com/data").unwrap();
        let mut proxy = Proxy::all("http://proxy.example.com:8080").unwrap();
        proxy.basic_auth("username", "password");

        let mut proxies = Proxies::default();
        proxies.add_proxy(proxy);

        let route = ProxyRoute::resolve(&proxies, &uri);
        assert_eq!(
            route.proxy_auth(),
            Some(String::from("dXNlcm5hbWU6cGFzc3dvcmQ="))
        );
    }

    #[cfg(feature = "__tls")]
    #[test]
    fn ut_proxy_route_https_proxy_with_tls_feature() {
        let uri = Uri::from_bytes(b"https://origin.example.com/data").unwrap();
        let mut proxies = Proxies::default();
        proxies.add_proxy(Proxy::all("https://proxy.example.com:8443").unwrap());

        let route = ProxyRoute::resolve(&proxies, &uri);
        let (_, scheme, authority) = route.pool_key().unwrap();

        assert!(route.is_proxied());
        assert_eq!(scheme.as_str(), "https");
        assert_eq!(authority.to_string(), "proxy.example.com:8443");
        assert!(matches!(route, ProxyRoute::Https(_)));
        route.ensure_supported().unwrap();
    }

    #[cfg(not(feature = "__tls"))]
    #[test]
    fn ut_proxy_route_https_without_tls_is_unsupported() {
        let uri = Uri::from_bytes(b"https://origin.example.com/data").unwrap();
        let mut proxies = Proxies::default();
        proxies.add_proxy(Proxy::all("https://proxy.example.com:8443").unwrap());

        let route = ProxyRoute::resolve(&proxies, &uri);
        assert!(route.is_proxied());
        assert!(matches!(route, ProxyRoute::HttpsUnsupported(_)));

        let err = route.ensure_supported().unwrap_err();
        assert_eq!(err.error_kind(), crate::ErrorKind::Connect);
        assert_eq!(
            format!("{}", err),
            "Connect Error: HTTPS proxy requires TLS feature"
        );
    }
}
