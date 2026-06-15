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

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(feature = "http3")]
use crate::async_impl::QuicConn;
use crate::{ConnProtocol, TimeGroup};

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

fn next_conn_id() -> u64 {
    // Relaxed is enough here: the ID is a correlation token, not a memory fence.
    NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed)
}

/// `ConnDetail` trait, which is used to obtain information about the current
/// connection.
pub trait ConnInfo {
    /// Whether the current connection is a proxy.
    fn is_proxy(&self) -> bool;

    /// Gets connection information data.
    fn conn_data(&self) -> ConnData;

    /// Sets the current transport phase when the implementation can persist it.
    fn set_transport_phase(&mut self, _phase: TransportPhase) {}

    /// Gets the current transport phase.
    fn transport_phase(&self) -> TransportPhase {
        self.conn_data().transport_phase()
    }

    /// Gets quic information
    #[cfg(feature = "http3")]
    fn quic_conn(&mut self) -> Option<QuicConn>;
}

/// High-level role of the connection transport stack.
///
/// This is intentionally coarser than the concrete stream type. Proxy CONNECT
/// performance work needs a stable place to attach phase and scheduling policy
/// without matching on connector internals or changing public APIs.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum TransportRole {
    #[default]
    DirectHttp,
    DirectHttps,
    HttpOverHttpProxy,
    HttpOverHttpsProxy,
    HttpsOverHttpProxy,
    HttpsOverHttpsProxy,
}

/// Coarse runtime phase for a transport stack.
///
/// The phase is an internal hint, not public protocol state. It gives the
/// client and, later, the runtime a stable vocabulary for distinguishing
/// latency-sensitive first-byte work from bulk body transfer.
#[doc(hidden)]
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TransportPhase {
    #[default]
    Connect,
    RequestWrite,
    FirstByteWait,
    HeaderDecode,
    BodyDrain,
    BodyDrainSmall,
    BodyDrainLarge,
}

/// Tcp connection information.
#[derive(Clone)]
pub struct ConnDetail {
    /// Transport layer protocol type.
    pub(crate) protocol: ConnProtocol,
    /// local socket address.
    pub(crate) local: SocketAddr,
    /// peer socket address.
    pub(crate) peer: SocketAddr,
    /// peer domain information.
    pub(crate) addr: String,
}

impl ConnDetail {
    /// Gets the transport layer protocol for the connection.
    pub fn protocol(&self) -> &ConnProtocol {
        &self.protocol
    }

    /// Gets the local socket address of the connection.
    pub fn local(&self) -> SocketAddr {
        self.local
    }

    /// Gets the peer socket address of the connection.
    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// Gets the peer domain address of the connection.
    pub fn addr(&self) -> &str {
        &self.addr
    }
}

/// Negotiated http version information.
#[derive(Default, Clone)]
pub struct NegotiateInfo {
    alpn: Option<Vec<u8>>,
}

impl NegotiateInfo {
    /// Constructs NegotiateInfo with apln extensions.
    #[cfg(feature = "__tls")]
    pub fn from_alpn(alpn: Option<Vec<u8>>) -> Self {
        Self { alpn }
    }

    /// tls alpn Indicates extended information.
    pub fn alpn(&self) -> Option<&[u8]> {
        self.alpn.as_deref()
    }
}

/// Transport layer connection establishment information data.
#[derive(Clone)]
pub struct ConnData {
    #[allow(dead_code)]
    conn_id: u64,
    detail: ConnDetail,
    #[cfg(feature = "http2")]
    negotiate: NegotiateInfo,
    proxy: bool,
    proxy_auth: Option<String>,
    #[allow(dead_code)]
    transport_role: TransportRole,
    #[allow(dead_code)]
    transport_phase: TransportPhase,
    time_group: TimeGroup,
}

impl ConnData {
    /// Construct a `ConnDataBuilder`.
    pub fn builder() -> ConnDataBuilder {
        ConnDataBuilder::default()
    }

    pub(crate) fn detail(self) -> ConnDetail {
        self.detail
    }

    #[cfg(feature = "http2")]
    pub(crate) fn negotiate(&self) -> &NegotiateInfo {
        &self.negotiate
    }

    pub(crate) fn is_proxy(&self) -> bool {
        self.proxy
    }

    pub(crate) fn proxy_auth(&self) -> Option<&str> {
        self.proxy_auth.as_deref()
    }

    #[allow(dead_code)]
    pub(crate) fn conn_id(&self) -> u64 {
        self.conn_id
    }

    #[allow(dead_code)]
    pub(crate) fn transport_role(&self) -> TransportRole {
        self.transport_role
    }

    #[allow(dead_code)]
    pub(crate) fn transport_phase(&self) -> TransportPhase {
        self.transport_phase
    }

    pub(crate) fn set_transport_phase(&mut self, phase: TransportPhase) {
        self.transport_phase = phase;
    }

    pub(crate) fn time_group_mut(&mut self) -> &mut TimeGroup {
        &mut self.time_group
    }
}

/// ConnData's builder, which builds ConnData through cascading calls.
#[derive(Default)]
pub struct ConnDataBuilder {
    conn_id: Option<u64>,
    #[cfg(feature = "http2")]
    negotiate: NegotiateInfo,
    proxy: bool,
    proxy_auth: Option<String>,
    transport_role: TransportRole,
    transport_phase: TransportPhase,
    time_group: TimeGroup,
}

impl ConnDataBuilder {
    /// Sets the http negotiation result.
    #[cfg(all(feature = "__tls", feature = "http2"))]
    pub fn negotiate(mut self, negotiate: NegotiateInfo) -> Self {
        self.negotiate = negotiate;
        self
    }

    /// Sets whether the peer is a proxy.
    pub fn proxy(mut self, proxy: bool) -> Self {
        self.proxy = proxy;
        self
    }

    pub(crate) fn proxy_auth(mut self, auth: Option<String>) -> Self {
        self.proxy_auth = auth;
        self
    }

    #[allow(dead_code)]
    pub(crate) fn conn_id(mut self, conn_id: u64) -> Self {
        self.conn_id = Some(conn_id);
        self
    }

    #[allow(dead_code)]
    pub(crate) fn transport_role(mut self, role: TransportRole) -> Self {
        self.transport_role = role;
        self
    }

    #[allow(dead_code)]
    pub(crate) fn transport_phase(mut self, phase: TransportPhase) -> Self {
        self.transport_phase = phase;
        self
    }

    /// Set the time required for each phase of connection establishment.
    pub fn time_group(mut self, time_group: TimeGroup) -> Self {
        self.time_group = time_group;
        self
    }

    /// Construct ConnData by setting the individual endpoint information.
    pub fn build(self, detail: ConnDetail) -> ConnData {
        ConnData {
            conn_id: self.conn_id.unwrap_or_else(next_conn_id),
            detail,
            #[cfg(feature = "http2")]
            negotiate: self.negotiate,
            proxy: self.proxy,
            proxy_auth: self.proxy_auth,
            transport_role: self.transport_role,
            transport_phase: self.transport_phase,
            time_group: self.time_group,
        }
    }
}

#[cfg(test)]
mod ut_conn_data {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use crate::ConnProtocol;

    use super::{ConnData, ConnDetail, TransportPhase, TransportRole};

    fn detail() -> ConnDetail {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 80);
        ConnDetail {
            protocol: ConnProtocol::Tcp,
            local: addr,
            peer: addr,
            addr: "127.0.0.1:80".to_string(),
        }
    }

    #[test]
    fn ut_conn_data_transport_role_default() {
        let data = ConnData::builder().build(detail());
        assert_eq!(data.transport_role(), TransportRole::DirectHttp);
    }

    #[test]
    fn ut_conn_data_conn_id_auto_assigns_unique_values() {
        let first = ConnData::builder().build(detail());
        let second = ConnData::builder().build(detail());
        assert_ne!(first.conn_id(), 0);
        assert_ne!(second.conn_id(), 0);
        assert_ne!(first.conn_id(), second.conn_id());
    }

    #[test]
    fn ut_conn_data_conn_id_setter_overrides_auto_assignment() {
        let data = ConnData::builder().conn_id(42).build(detail());
        assert_eq!(data.conn_id(), 42);
    }

    #[test]
    fn ut_conn_data_transport_role_setter() {
        let data = ConnData::builder()
            .transport_role(TransportRole::HttpsOverHttpsProxy)
            .build(detail());
        assert_eq!(data.transport_role(), TransportRole::HttpsOverHttpsProxy);
    }

    #[test]
    fn ut_conn_data_transport_phase_default_and_setter() {
        let mut data = ConnData::builder().build(detail());
        assert_eq!(data.transport_phase(), TransportPhase::Connect);

        data.set_transport_phase(TransportPhase::FirstByteWait);
        assert_eq!(data.transport_phase(), TransportPhase::FirstByteWait);

        let data = ConnData::builder()
            .transport_phase(TransportPhase::BodyDrainLarge)
            .build(detail());
        assert_eq!(data.transport_phase(), TransportPhase::BodyDrainLarge);
    }
}
