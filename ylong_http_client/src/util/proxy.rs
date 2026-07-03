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

//! Proxy implementation.

use core::convert::TryFrom;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};

use ylong_http::headers::HeaderValue;
use ylong_http::request::uri::{Authority, Scheme, Uri};

use crate::error::HttpClientError;
use crate::util::base64::encode;
use crate::util::normalizer::UriFormatter;
#[cfg(test)]
use crate::util::pool::PoolKey;

static NEXT_PROXY_POOL_ID: AtomicU64 = AtomicU64::new(1);

/// `Proxies` is responsible for managing a list of proxies.
#[derive(Clone, Default)]
pub(crate) struct Proxies {
    list: Vec<Proxy>,
}

impl Proxies {
    pub(crate) fn add_proxy(&mut self, proxy: Proxy) {
        self.list.push(proxy)
    }

    pub(crate) fn match_proxy(&self, uri: &Uri) -> Option<&Proxy> {
        self.list.iter().find(|proxy| proxy.is_intercepted(uri))
    }

    #[cfg(test)]
    pub(crate) fn pool_key(&self, uri: &Uri) -> PoolKey {
        match self.match_proxy(uri) {
            Some(proxy) => proxy.pool_key(uri),
            None => {
                // Pool keys are built after request URI normalization, the same
                // invariant required by proxy matching.
                PoolKey::new(
                    uri.scheme().unwrap().clone(),
                    uri.authority().unwrap().clone(),
                )
            }
        }
    }

    pub(crate) fn proxy_pool_key(&self, uri: &Uri) -> Option<(u64, Scheme, Authority)> {
        self.match_proxy(uri).map(Proxy::proxy_pool_key)
    }
}

/// Proxy is a configuration of client which should manage the destination
/// address of request.
///
/// A `Proxy` has below rules:
///
/// - Manage the uri of destination address.
/// - Manage the request content such as headers.
/// - Provide no proxy function which the request will not affected by proxy.
#[derive(Clone)]
pub(crate) struct Proxy {
    pub(crate) intercept: Intercept,
    pub(crate) no_proxy: Option<NoProxy>,
}

impl Proxy {
    pub(crate) fn new(intercept: Intercept) -> Self {
        Self {
            intercept,
            no_proxy: None,
        }
    }

    pub(crate) fn http(uri: &str) -> Result<Self, HttpClientError> {
        Ok(Proxy::new(Intercept::Http(ProxyInfo::new(uri)?)))
    }

    pub(crate) fn https(uri: &str) -> Result<Self, HttpClientError> {
        Ok(Proxy::new(Intercept::Https(ProxyInfo::new(uri)?)))
    }

    pub(crate) fn all(uri: &str) -> Result<Self, HttpClientError> {
        Ok(Proxy::new(Intercept::All(ProxyInfo::new(uri)?)))
    }

    pub(crate) fn basic_auth(&mut self, username: &str, password: &str) {
        let auth = encode(format!("{username}:{password}").as_bytes());

        // All characters in base64 format are valid characters, so we ignore the error.
        let mut auth = HeaderValue::from_bytes(auth.as_slice()).unwrap();
        auth.set_sensitive(true);

        match &mut self.intercept {
            Intercept::All(info) => info.basic_auth = Some(auth),
            Intercept::Http(info) => info.basic_auth = Some(auth),
            Intercept::Https(info) => info.basic_auth = Some(auth),
        }
    }

    pub(crate) fn no_proxy(&mut self, no_proxy: &str) {
        self.no_proxy = NoProxy::from_str(no_proxy);
    }

    #[cfg_attr(not(any(test, feature = "sync")), allow(dead_code))]
    pub(crate) fn via_proxy(&self, uri: &Uri) -> Uri {
        let info = self.intercept.proxy_info();
        let mut builder = Uri::builder();
        builder = builder
            .scheme(info.scheme().clone())
            .authority(info.authority().clone());

        if let Some(path) = uri.path() {
            builder = builder.path(path.clone());
        }

        if let Some(query) = uri.query() {
            builder = builder.query(query.clone());
        }

        // Here all parts of builder is accurate.
        builder.build().unwrap()
    }

    pub(crate) fn is_intercepted(&self, uri: &Uri) -> bool {
        // uri is formatted uri, use unwrap directly
        let no_proxy = self
            .no_proxy
            .as_ref()
            .map(|no_proxy| no_proxy.contain(uri.host().unwrap().as_str()))
            .unwrap_or(false);

        match self.intercept {
            Intercept::All(_) => !no_proxy,
            Intercept::Http(_) => !no_proxy && *uri.scheme().unwrap() == Scheme::HTTP,
            Intercept::Https(_) => !no_proxy && *uri.scheme().unwrap() == Scheme::HTTPS,
        }
    }

    #[cfg(test)]
    fn pool_key(&self, uri: &Uri) -> PoolKey {
        let (id, scheme, authority) = self.proxy_pool_key();
        // Pool keys are built after request URI normalization, the same
        // invariant required by proxy matching.
        PoolKey::proxied(
            uri.scheme().unwrap().clone(),
            uri.authority().unwrap().clone(),
            id,
            scheme,
            authority,
        )
    }

    fn proxy_pool_key(&self) -> (u64, Scheme, Authority) {
        let info = self.intercept.proxy_info();
        (
            info.pool_key_id(),
            info.scheme().clone(),
            info.authority().clone(),
        )
    }
}

#[derive(Clone)]
pub(crate) enum Intercept {
    All(ProxyInfo),
    Http(ProxyInfo),
    Https(ProxyInfo),
}

impl Intercept {
    pub(crate) fn proxy_info(&self) -> &ProxyInfo {
        match self {
            Self::All(info) => info,
            Self::Http(info) => info,
            Self::Https(info) => info,
        }
    }
}

/// ProxyInfo which contains authentication, scheme and host.
#[derive(Clone)]
pub(crate) struct ProxyInfo {
    pool_key_id: u64,
    pub(crate) scheme: Scheme,
    pub(crate) authority: Authority,
    pub(crate) basic_auth: Option<HeaderValue>,
    #[cfg(feature = "__tls")]
    pub(crate) tls_config: Option<crate::util::TlsConfig>,
}

impl ProxyInfo {
    pub(crate) fn new(uri: &str) -> Result<Self, HttpClientError> {
        let mut uri = match Uri::try_from(uri) {
            Ok(u) => u,
            Err(e) => {
                return err_from_other!(Build, e);
            }
        };
        // Makes sure that all parts of uri exist.
        UriFormatter::new().format(&mut uri)?;
        let (scheme, authority, _, _) = uri.into_parts();
        // `scheme` and `authority` must have values after formatting.
        // Relaxed ordering is sufficient: this counter only allocates distinct
        // pool-key identities and does not synchronize access to proxy data.
        let pool_key_id = NEXT_PROXY_POOL_ID.fetch_add(1, Ordering::Relaxed);
        Ok(Self {
            pool_key_id,
            basic_auth: None,
            scheme: scheme.unwrap(),
            authority: authority.unwrap(),
            #[cfg(feature = "__tls")]
            tls_config: None,
        })
    }

    pub(crate) fn authority(&self) -> &Authority {
        &self.authority
    }

    pub(crate) fn scheme(&self) -> &Scheme {
        &self.scheme
    }

    pub(crate) fn pool_key_id(&self) -> u64 {
        self.pool_key_id
    }
}

#[derive(Clone)]
enum Ip {
    Address(IpAddr),
}

#[derive(Clone, Default)]
pub(crate) struct NoProxy {
    ips: Vec<Ip>,
    domains: Vec<String>,
}

impl NoProxy {
    pub(crate) fn from_str(no_proxy: &str) -> Option<Self> {
        if no_proxy.is_empty() {
            return None;
        }

        let no_proxy_vec = no_proxy.split(',').map(|c| c.trim()).collect::<Vec<&str>>();
        let mut ip_list = Vec::new();
        let mut domains_list = Vec::new();

        for host in no_proxy_vec {
            let address = match Uri::from_bytes(host.as_bytes()) {
                Ok(uri) => uri,
                Err(_) => {
                    continue;
                }
            };
            // use unwrap directly, host has been checked before
            match address.host().unwrap().as_str().parse::<IpAddr>() {
                Ok(ip) => ip_list.push(Ip::Address(ip)),
                Err(_) => domains_list.push(host.to_string()),
            }
        }
        Some(NoProxy {
            ips: ip_list,
            domains: domains_list,
        })
    }

    pub(crate) fn contain(&self, proxy_host: &str) -> bool {
        match proxy_host.parse::<IpAddr>() {
            Ok(ip) => self.contains_ip(ip),
            Err(_) => self.contains_domain(proxy_host),
        }
    }

    fn contains_ip(&self, ip: IpAddr) -> bool {
        for Ip::Address(i) in self.ips.iter() {
            if &ip == i {
                return true;
            }
        }
        false
    }

    fn contains_domain(&self, domain: &str) -> bool {
        for block_domain in self.domains.iter() {
            let mut block_domain = block_domain.clone();
            // Changes *.example.com to .example.com
            if (block_domain.starts_with('*')) && (block_domain.len() > 1) {
                block_domain = block_domain.trim_matches('*').to_string();
            }

            if block_domain == "*"
                || block_domain.ends_with(domain)
                || block_domain == domain
                || block_domain.trim_matches('.') == domain
            {
                return true;
            } else if domain.ends_with(&block_domain) {
                // .example.com and www.
                if block_domain.starts_with('.')
                    || domain.as_bytes().get(domain.len() - block_domain.len() - 1) == Some(&b'.')
                {
                    return true;
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod ut_proxy {
    use ylong_http::request::uri::{Scheme, Uri};

    use crate::util::proxy::{Proxies, Proxy};

    /// UT test cases for `Proxy::via_proxy`.
    ///
    /// # Brief
    /// 1. Creates a `Proxy`.
    /// 2. Calls `Proxy::via_proxy` with some `Uri`to get the results.
    /// 4. Checks if the test result is correct.
    #[test]
    fn ut_via_proxy() {
        let proxy = Proxy::http("http://www.example.com").unwrap();
        let uri = Uri::from_bytes(b"http://www.example2.com").unwrap();
        let res = proxy.via_proxy(&uri);
        assert_eq!(res.to_string(), "http://www.example.com:80");
    }

    /// UT test cases for `Proxies`.
    ///
    /// # Brief
    /// 1. Creates a `Proxies`.
    /// 2. Adds some `Proxy` to `Proxies`
    /// 3. Calls `Proxies::match_proxy` with some `Uri`s and get the results.
    /// 4. Checks if the test result is correct.
    #[test]
    fn ut_proxies() {
        let mut proxies = Proxies::default();
        proxies.add_proxy(Proxy::http("http://www.aaa.com").unwrap());
        proxies.add_proxy(Proxy::https("http://www.bbb.com").unwrap());

        let uri = Uri::from_bytes(b"http://www.example.com").unwrap();
        let proxy = proxies.match_proxy(&uri).unwrap();
        assert!(proxy.no_proxy.is_none());
        let info = proxy.intercept.proxy_info();
        assert_eq!(info.scheme, Scheme::HTTP);
        assert_eq!(info.authority.to_string(), "www.aaa.com:80");

        let uri = Uri::from_bytes(b"https://www.example.com").unwrap();
        let matched = proxies.match_proxy(&uri).unwrap();
        assert!(matched.no_proxy.is_none());
        let info = matched.intercept.proxy_info();
        assert_eq!(info.scheme, Scheme::HTTP);
        assert_eq!(info.authority.to_string(), "www.bbb.com:80");

        // with no_proxy
        let mut proxies = Proxies::default();
        let mut proxy = Proxy::http("http://www.aaa.com").unwrap();
        proxy.no_proxy("http://no_proxy.aaa.com");
        proxies.add_proxy(proxy);

        let uri = Uri::from_bytes(b"http://www.bbb.com").unwrap();
        let matched = proxies.match_proxy(&uri).unwrap();
        let info = matched.intercept.proxy_info();
        assert_eq!(info.scheme, Scheme::HTTP);
        assert_eq!(info.authority.to_string(), "www.aaa.com:80");

        let uri = Uri::from_bytes(b"http://no_proxy.aaa.com").unwrap();
        assert!(proxies.match_proxy(&uri).is_none());

        let mut proxies = Proxies::default();
        let mut proxy = Proxy::http("http://www.aaa.com").unwrap();
        proxy.no_proxy(".aaa.com");
        proxies.add_proxy(proxy);

        let uri = Uri::from_bytes(b"http://no_proxy.aaa.com").unwrap();
        assert!(proxies.match_proxy(&uri).is_none());

        let mut proxies = Proxies::default();
        let mut proxy = Proxy::http("http://127.0.0.1:3000").unwrap();
        proxy.no_proxy("http://127.0.0.1:80");
        proxies.add_proxy(proxy);

        let uri = Uri::from_bytes(b"http://127.0.0.1:80").unwrap();
        assert!(proxies.match_proxy(&uri).is_none());
    }

    /// UT test cases for HTTP proxy pool keys.
    ///
    /// # Brief
    /// 1. Creates direct and HTTP-proxied keys for the same target URI.
    /// 2. Checks if HTTP proxy matching and URI rewriting are unchanged.
    /// 3. Checks if no_proxy uses the direct pool key.
    #[test]
    fn ut_http_proxy_pool_key() {
        let uri = Uri::from_bytes(b"http://www.example.com/path").unwrap();
        let direct = Proxies::default().pool_key(&uri);

        let mut proxies = Proxies::default();
        let proxy = Proxy::http("http://proxy.example.com").unwrap();
        assert_eq!(
            proxy.via_proxy(&uri).to_string(),
            "http://proxy.example.com:80/path"
        );
        proxies.add_proxy(proxy);

        assert!(proxies.match_proxy(&uri).is_some());
        assert_ne!(direct, proxies.pool_key(&uri));

        let mut no_proxy = Proxies::default();
        let mut proxy = Proxy::http("http://proxy.example.com").unwrap();
        proxy.no_proxy("http://www.example.com");
        no_proxy.add_proxy(proxy);
        assert!(no_proxy.match_proxy(&uri).is_none());
        assert_eq!(direct, no_proxy.pool_key(&uri));
    }

    /// UT test cases for HTTPS proxy TLS pool keys.
    ///
    /// # Brief
    /// 1. Creates two HTTPS proxy configurations with different TLS settings.
    /// 2. Checks if the same target URI gets different pool keys.
    /// 3. Checks if cloning preserves the proxy pool key identity.
    #[cfg(feature = "__tls")]
    #[test]
    fn ut_https_proxy_tls_pool_key() {
        let uri = Uri::from_bytes(b"https://www.example.com/path").unwrap();
        let tls_a = crate::util::TlsConfig::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();
        let tls_b = crate::util::TlsConfig::builder()
            .danger_accept_invalid_hostnames(true)
            .build()
            .unwrap();

        let mut proxies_a = Proxies::default();
        proxies_a.add_proxy(
            crate::Proxy::https("https://proxy.example.com:8443")
                .proxy_tls_config(tls_a)
                .build()
                .unwrap()
                .inner(),
        );

        let mut proxies_b = Proxies::default();
        proxies_b.add_proxy(
            crate::Proxy::https("https://proxy.example.com:8443")
                .proxy_tls_config(tls_b)
                .build()
                .unwrap()
                .inner(),
        );

        let key_a = proxies_a.pool_key(&uri);
        assert_ne!(key_a, proxies_b.pool_key(&uri));
        assert_eq!(key_a, proxies_a.clone().pool_key(&uri));
    }
}
