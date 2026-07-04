use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub(crate) struct Metrics {
    proxy_client_connections: AtomicU64,
    proxy_tls_handshakes: AtomicU64,
    connect_requests: AtomicU64,
    origin_tcp_connections: AtomicU64,
    origin_tls_handshakes: AtomicU64,
    forwarded_requests: AtomicU64,
    origin_policy_closes: AtomicU64,
    origin_small_responses: AtomicU64,
    origin_medium_responses: AtomicU64,
    origin_large_responses: AtomicU64,
    bytes_client_to_origin: AtomicU64,
    bytes_origin_to_client: AtomicU64,
    proxy_errors: AtomicU64,
    origin_errors: AtomicU64,
}

impl Metrics {
    // These counters are diagnostic only and are read after Ctrl-C for a final
    // fixture snapshot. They do not synchronize protocol state, so relaxed
    // ordering is sufficient.
    pub(crate) fn inc_proxy_client_connections(&self) {
        self.proxy_client_connections
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn inc_proxy_tls_handshakes(&self) {
        self.proxy_tls_handshakes.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn inc_connect_requests(&self) {
        self.connect_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn inc_origin_tcp_connections(&self) {
        self.origin_tcp_connections.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn inc_origin_tls_handshakes(&self) {
        self.origin_tls_handshakes.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn inc_forwarded_requests(&self) {
        self.forwarded_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn inc_origin_policy_closes(&self) {
        self.origin_policy_closes.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_origin_response_size(&self, bytes: usize) {
        let counter = if bytes <= 4 * 1024 {
            &self.origin_small_responses
        } else if bytes <= 64 * 1024 {
            &self.origin_medium_responses
        } else {
            &self.origin_large_responses
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn add_client_to_origin_bytes(&self, bytes: u64) {
        self.bytes_client_to_origin
            .fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn add_origin_to_client_bytes(&self, bytes: u64) {
        self.bytes_origin_to_client
            .fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn inc_proxy_errors(&self) {
        self.proxy_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn inc_origin_errors(&self) {
        self.origin_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn json_snapshot(&self) -> String {
        format!(
            "{{\"event\":\"fixture_metrics\",\"proxy_client_connections\":{},\"proxy_tls_handshakes\":{},\"connect_requests\":{},\"origin_tcp_connections\":{},\"origin_tls_handshakes\":{},\"forwarded_requests\":{},\"origin_policy_closes\":{},\"origin_small_responses\":{},\"origin_medium_responses\":{},\"origin_large_responses\":{},\"bytes_client_to_origin\":{},\"bytes_origin_to_client\":{},\"proxy_errors\":{},\"origin_errors\":{}}}",
            self.proxy_client_connections.load(Ordering::Relaxed),
            self.proxy_tls_handshakes.load(Ordering::Relaxed),
            self.connect_requests.load(Ordering::Relaxed),
            self.origin_tcp_connections.load(Ordering::Relaxed),
            self.origin_tls_handshakes.load(Ordering::Relaxed),
            self.forwarded_requests.load(Ordering::Relaxed),
            self.origin_policy_closes.load(Ordering::Relaxed),
            self.origin_small_responses.load(Ordering::Relaxed),
            self.origin_medium_responses.load(Ordering::Relaxed),
            self.origin_large_responses.load(Ordering::Relaxed),
            self.bytes_client_to_origin.load(Ordering::Relaxed),
            self.bytes_origin_to_client.load(Ordering::Relaxed),
            self.proxy_errors.load(Ordering::Relaxed),
            self.origin_errors.load(Ordering::Relaxed)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::Metrics;

    #[test]
    fn json_snapshot_contains_counter_values() {
        let metrics = Metrics::default();
        metrics.inc_proxy_client_connections();
        metrics.inc_proxy_tls_handshakes();
        metrics.inc_connect_requests();
        metrics.inc_forwarded_requests();
        metrics.inc_origin_policy_closes();
        metrics.record_origin_response_size(1024);
        metrics.record_origin_response_size(16 * 1024);
        metrics.record_origin_response_size(256 * 1024);
        metrics.add_client_to_origin_bytes(7);
        metrics.add_origin_to_client_bytes(11);

        let json = metrics.json_snapshot();

        assert!(json.contains("\"proxy_client_connections\":1"));
        assert!(json.contains("\"proxy_tls_handshakes\":1"));
        assert!(json.contains("\"connect_requests\":1"));
        assert!(json.contains("\"forwarded_requests\":1"));
        assert!(json.contains("\"origin_policy_closes\":1"));
        assert!(json.contains("\"origin_small_responses\":1"));
        assert!(json.contains("\"origin_medium_responses\":1"));
        assert!(json.contains("\"origin_large_responses\":1"));
        assert!(json.contains("\"bytes_client_to_origin\":7"));
        assert!(json.contains("\"bytes_origin_to_client\":11"));
    }
}
