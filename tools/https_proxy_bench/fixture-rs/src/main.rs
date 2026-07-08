mod config;
mod http;
mod metrics;
mod origin;
mod proxy;
mod relay;
mod tls;

use std::process::ExitCode;
use std::sync::Arc;

use config::Config;
use metrics::Metrics;

#[tokio::main]
async fn main() -> ExitCode {
    let config = match Config::parse() {
        Ok(config) => config,
        Err(err) if err.starts_with("usage:") => {
            println!("{err}");
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    };

    let metrics = Arc::new(Metrics::default());
    let origin_server = match origin::bind(config.clone(), Arc::clone(&metrics)).await {
        Ok(server) => server,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };
    let proxy_server = match proxy::bind(config.clone(), Arc::clone(&metrics)).await {
        Ok(server) => server,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };

    println!("{}", fixture_start_json(&config));

    let origin_task = tokio::spawn(async move { origin_server.run().await });
    let proxy_task = tokio::spawn(async move { proxy_server.run().await });

    tokio::select! {
        signal = tokio::signal::ctrl_c() => {
            if let Err(err) = signal {
                eprintln!("failed to wait for Ctrl-C: {err}");
                return ExitCode::FAILURE;
            }
        }
        result = origin_task => {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    eprintln!("origin failed: {err}");
                    return ExitCode::FAILURE;
                }
                Err(err) => {
                    eprintln!("origin task failed: {err}");
                    return ExitCode::FAILURE;
                }
            }
        }
        result = proxy_task => {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    eprintln!("proxy failed: {err}");
                    return ExitCode::FAILURE;
                }
                Err(err) => {
                    eprintln!("proxy task failed: {err}");
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    println!("{}", metrics.json_snapshot());
    ExitCode::SUCCESS
}

fn fixture_start_json(config: &Config) -> String {
    let origin_scheme = if config.origin_tls { "https" } else { "http" };
    let origin_url = format!(
        "{}://{}:{}/",
        origin_scheme, config.origin_host, config.origin_port
    );
    let https_proxy = format!("https://{}:{}", config.proxy_host, config.proxy_port);
    let origin_close = config
        .origin_close_every_n_requests
        .map_or_else(|| "null".to_string(), |value| value.to_string());

    format!(
        "{{\"event\":\"fixture_start\",\"origin_url\":\"{}\",\"https_proxy\":\"{}\",\"response_size\":{},\"response_size_sequence\":{},\"origin_tls\":{},\"origin_http2\":{},\"origin_close_every_n_requests\":{}}}",
        json_escape(&origin_url),
        json_escape(&https_proxy),
        config.response_size,
        json_usize_array(&config.response_size_sequence),
        config.origin_tls,
        config.origin_http2,
        origin_close
    )
}

fn json_usize_array(values: &[usize]) -> String {
    let mut out = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&value.to_string());
    }
    out.push(']');
    out
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0c}' => escaped.push_str("\\f"),
            ch if ch.is_control() => {
                use std::fmt::Write;
                let _ = write!(escaped, "\\u{:04x}", ch as u32);
            }
            ch => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::{fixture_start_json, Config};

    fn test_config() -> Config {
        Config {
            cert_file: "server.pem".to_string(),
            key_file: "server.key".to_string(),
            ca_file: None,
            require_client_cert: false,
            origin_tls: false,
            origin_http2: false,
            origin_host: "origin.test".to_string(),
            origin_port: 18080,
            proxy_host: "proxy.test".to_string(),
            proxy_port: 18443,
            response_size: 1024,
            response_size_sequence: Vec::new(),
            relay_buffer_size: 16 * 1024,
            origin_delay_ms: 0,
            origin_close_every_n_requests: None,
            tls_groups: None,
        }
    }

    #[test]
    fn startup_json_uses_configured_proxy_host() {
        let config = test_config();

        let json = fixture_start_json(&config);

        assert!(json.contains("\"origin_url\":\"http://origin.test:18080/\""));
        assert!(json.contains("\"https_proxy\":\"https://proxy.test:18443\""));
        assert!(json.contains("\"response_size_sequence\":[]"));
        assert!(json.contains("\"origin_http2\":false"));
        assert!(json.contains("\"origin_close_every_n_requests\":null"));
    }

    #[test]
    fn startup_json_escapes_quote_and_backslash_in_string_fields() {
        let mut config = test_config();
        config.origin_host = "origin\\host\"name".to_string();
        config.proxy_host = "proxy\\host\"name".to_string();

        let json = fixture_start_json(&config);

        assert!(json.contains("\"origin_url\":\"http://origin\\\\host\\\"name:18080/\""));
        assert!(json.contains("\"https_proxy\":\"https://proxy\\\\host\\\"name:18443\""));
    }

    #[test]
    fn startup_json_includes_scenario_controls() {
        let mut config = test_config();
        config.response_size_sequence = vec![1024, 4096, 65536];
        config.origin_tls = true;
        config.origin_http2 = true;
        config.origin_close_every_n_requests = Some(100);

        let json = fixture_start_json(&config);

        assert!(json.contains("\"response_size_sequence\":[1024,4096,65536]"));
        assert!(json.contains("\"origin_http2\":true"));
        assert!(json.contains("\"origin_close_every_n_requests\":100"));
    }
}
