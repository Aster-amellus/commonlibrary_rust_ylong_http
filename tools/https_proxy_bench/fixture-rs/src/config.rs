#[derive(Clone, Debug)]
pub(crate) struct Config {
    pub(crate) cert_file: String,
    pub(crate) key_file: String,
    pub(crate) ca_file: Option<String>,
    pub(crate) require_client_cert: bool,
    pub(crate) origin_tls: bool,
    pub(crate) origin_http2: bool,
    pub(crate) origin_host: String,
    pub(crate) origin_port: u16,
    pub(crate) proxy_host: String,
    pub(crate) proxy_port: u16,
    pub(crate) response_size: usize,
    pub(crate) response_size_sequence: Vec<usize>,
    pub(crate) relay_buffer_size: usize,
    pub(crate) origin_delay_ms: u64,
    pub(crate) origin_close_every_n_requests: Option<usize>,
    pub(crate) tls_groups: Option<String>,
}

impl Config {
    pub(crate) fn parse() -> Result<Self, String> {
        Self::parse_from(std::env::args().collect())
    }

    pub(crate) fn parse_from(args: Vec<String>) -> Result<Self, String> {
        let mut config = Self {
            cert_file: String::new(),
            key_file: String::new(),
            ca_file: None,
            require_client_cert: false,
            origin_tls: false,
            origin_http2: false,
            origin_host: "127.0.0.1".to_string(),
            origin_port: 18080,
            proxy_host: "127.0.0.1".to_string(),
            proxy_port: 18443,
            response_size: 1024,
            response_size_sequence: Vec::new(),
            relay_buffer_size: 16 * 1024,
            origin_delay_ms: 0,
            origin_close_every_n_requests: None,
            tls_groups: None,
        };

        let mut index = 1;
        while index < args.len() {
            match args[index].as_str() {
                "--cert-file" => config.cert_file = next_value(&args, &mut index, "--cert-file")?,
                "--key-file" => config.key_file = next_value(&args, &mut index, "--key-file")?,
                "--ca-file" => config.ca_file = Some(next_value(&args, &mut index, "--ca-file")?),
                "--require-client-cert" => config.require_client_cert = true,
                "--origin-tls" => config.origin_tls = true,
                "--origin-http2" => config.origin_http2 = true,
                "--origin-host" => {
                    config.origin_host = next_value(&args, &mut index, "--origin-host")?
                }
                "--origin-port" => {
                    config.origin_port = parse_u16(next_value(&args, &mut index, "--origin-port")?)?
                }
                "--proxy-host" => {
                    config.proxy_host = next_value(&args, &mut index, "--proxy-host")?
                }
                "--proxy-port" => {
                    config.proxy_port = parse_u16(next_value(&args, &mut index, "--proxy-port")?)?
                }
                "--response-size" => {
                    config.response_size =
                        parse_usize(next_value(&args, &mut index, "--response-size")?)?
                }
                "--response-size-sequence" => {
                    config.response_size_sequence = parse_usize_sequence(next_value(
                        &args,
                        &mut index,
                        "--response-size-sequence",
                    )?)?
                }
                "--relay-buffer-size" => {
                    config.relay_buffer_size =
                        parse_usize(next_value(&args, &mut index, "--relay-buffer-size")?)?
                }
                "--origin-delay-ms" => {
                    config.origin_delay_ms =
                        parse_u64(next_value(&args, &mut index, "--origin-delay-ms")?)?
                }
                "--origin-close-every-n-requests" => {
                    let value = parse_usize(next_value(
                        &args,
                        &mut index,
                        "--origin-close-every-n-requests",
                    )?)?;
                    if value == 0 {
                        return Err("--origin-close-every-n-requests must be > 0".to_string());
                    }
                    config.origin_close_every_n_requests = Some(value);
                }
                "--tls-groups" => {
                    config.tls_groups = Some(next_value(&args, &mut index, "--tls-groups")?)
                }
                "--help" | "-h" => return Err(usage()),
                other => return Err(format!("unknown argument: {other}")),
            }
            index += 1;
        }

        if config.cert_file.is_empty() {
            return Err("--cert-file is required".to_string());
        }
        if config.key_file.is_empty() {
            return Err("--key-file is required".to_string());
        }
        if config.require_client_cert && config.ca_file.is_none() {
            return Err("--require-client-cert requires --ca-file".to_string());
        }
        if config.origin_http2 && !config.origin_tls {
            return Err("--origin-http2 requires --origin-tls".to_string());
        }
        if config.response_size == 0 {
            return Err("--response-size must be > 0".to_string());
        }
        if config.relay_buffer_size == 0 {
            return Err("--relay-buffer-size must be > 0".to_string());
        }

        Ok(config)
    }
}

fn next_value(args: &[String], index: &mut usize, name: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{name} requires a value"))
}

fn parse_u16(value: String) -> Result<u16, String> {
    value
        .parse::<u16>()
        .map_err(|_| format!("invalid u16: {value}"))
}

fn parse_usize(value: String) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("invalid usize: {value}"))
}

fn parse_usize_sequence(value: String) -> Result<Vec<usize>, String> {
    let mut sizes = Vec::new();
    for raw in value.split(',') {
        if raw.is_empty() {
            return Err(
                "--response-size-sequence must contain at least one positive integer".to_string(),
            );
        }
        let size = parse_usize(raw.to_string())?;
        if size == 0 {
            return Err(
                "--response-size-sequence must contain at least one positive integer".to_string(),
            );
        }
        sizes.push(size);
    }
    if sizes.is_empty() {
        return Err(
            "--response-size-sequence must contain at least one positive integer".to_string(),
        );
    }
    Ok(sizes)
}

fn parse_u64(value: String) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("invalid u64: {value}"))
}

fn usage() -> String {
    "usage: https_proxy_bench_fixture --cert-file PEM --key-file PEM [--ca-file PEM] [--require-client-cert] [--origin-tls] [--origin-http2] [--origin-host HOST] [--origin-port PORT] [--proxy-host HOST] [--proxy-port PORT] [--response-size N] [--response-size-sequence N,N,...] [--relay-buffer-size N] [--origin-delay-ms N] [--origin-close-every-n-requests N] [--tls-groups LIST]".to_string()
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn parses_required_tls_files_and_defaults() {
        let args = vec![
            "fixture".to_string(),
            "--cert-file".to_string(),
            "server.pem".to_string(),
            "--key-file".to_string(),
            "server.key".to_string(),
        ];

        let config = Config::parse_from(args).unwrap();

        assert_eq!(config.cert_file, "server.pem");
        assert_eq!(config.key_file, "server.key");
        assert_eq!(config.origin_host, "127.0.0.1");
        assert_eq!(config.origin_port, 18080);
        assert_eq!(config.proxy_host, "127.0.0.1");
        assert_eq!(config.proxy_port, 18443);
        assert_eq!(config.response_size, 1024);
        assert!(config.response_size_sequence.is_empty());
        assert_eq!(config.relay_buffer_size, 16 * 1024);
        assert_eq!(config.origin_close_every_n_requests, None);
        assert!(!config.origin_tls);
        assert!(!config.require_client_cert);
    }

    #[test]
    fn rejects_mtls_without_ca_file() {
        let args = vec![
            "fixture".to_string(),
            "--cert-file".to_string(),
            "server.pem".to_string(),
            "--key-file".to_string(),
            "server.key".to_string(),
            "--require-client-cert".to_string(),
        ];

        let err = Config::parse_from(args).unwrap_err();

        assert_eq!(err, "--require-client-cert requires --ca-file");
    }

    #[test]
    fn parses_response_size_sequence_and_origin_close_policy() {
        let args = vec![
            "fixture".to_string(),
            "--cert-file".to_string(),
            "server.pem".to_string(),
            "--key-file".to_string(),
            "server.key".to_string(),
            "--response-size-sequence".to_string(),
            "1024,4096,65536".to_string(),
            "--origin-close-every-n-requests".to_string(),
            "100".to_string(),
        ];

        let config = Config::parse_from(args).unwrap();

        assert_eq!(config.response_size_sequence, vec![1024, 4096, 65536]);
        assert_eq!(config.origin_close_every_n_requests, Some(100));
    }

    #[test]
    fn parses_tls_groups() {
        let args = vec![
            "fixture".to_string(),
            "--cert-file".to_string(),
            "server.pem".to_string(),
            "--key-file".to_string(),
            "server.key".to_string(),
            "--tls-groups".to_string(),
            "X25519".to_string(),
        ];

        let config = Config::parse_from(args).unwrap();

        assert_eq!(config.tls_groups.as_deref(), Some("X25519"));
    }

    #[test]
    fn parses_origin_http2() {
        let args = vec![
            "fixture".to_string(),
            "--cert-file".to_string(),
            "server.pem".to_string(),
            "--key-file".to_string(),
            "server.key".to_string(),
            "--origin-tls".to_string(),
            "--origin-http2".to_string(),
        ];

        let config = Config::parse_from(args).unwrap();

        assert!(config.origin_http2);
    }

    #[test]
    fn rejects_origin_http2_without_origin_tls() {
        let args = vec![
            "fixture".to_string(),
            "--cert-file".to_string(),
            "server.pem".to_string(),
            "--key-file".to_string(),
            "server.key".to_string(),
            "--origin-http2".to_string(),
        ];

        let err = Config::parse_from(args).unwrap_err();

        assert_eq!(err, "--origin-http2 requires --origin-tls");
    }

    #[test]
    fn rejects_empty_response_size_sequence() {
        let args = vec![
            "fixture".to_string(),
            "--cert-file".to_string(),
            "server.pem".to_string(),
            "--key-file".to_string(),
            "server.key".to_string(),
            "--response-size-sequence".to_string(),
            "".to_string(),
        ];

        let err = Config::parse_from(args).unwrap_err();

        assert_eq!(
            err,
            "--response-size-sequence must contain at least one positive integer"
        );
    }

    #[test]
    fn rejects_zero_origin_close_interval() {
        let args = vec![
            "fixture".to_string(),
            "--cert-file".to_string(),
            "server.pem".to_string(),
            "--key-file".to_string(),
            "server.key".to_string(),
            "--origin-close-every-n-requests".to_string(),
            "0".to_string(),
        ];

        let err = Config::parse_from(args).unwrap_err();

        assert_eq!(err, "--origin-close-every-n-requests must be > 0");
    }
}
