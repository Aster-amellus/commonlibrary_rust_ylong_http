#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RequestTarget {
    Connect {
        host: String,
        port: u16,
    },
    Absolute {
        scheme: String,
        host: String,
        port: u16,
        path: String,
    },
    OriginForm {
        path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedRequest {
    pub(crate) method: String,
    pub(crate) target: RequestTarget,
    pub(crate) version: String,
    pub(crate) content_length: usize,
}

pub(crate) fn header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
}

pub(crate) fn parse_request_header(header: &[u8]) -> Result<ParsedRequest, String> {
    let header =
        std::str::from_utf8(header).map_err(|err| format!("invalid header utf8: {err}"))?;
    let mut lines = header.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| "missing request line".to_string())?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| "missing method".to_string())?;
    let raw_target = request_parts
        .next()
        .ok_or_else(|| "missing request target".to_string())?;
    let version = request_parts
        .next()
        .ok_or_else(|| "missing http version".to_string())?;
    if request_parts.next().is_some() {
        return Err("invalid request line".to_string());
    }

    let mut content_length = 0;
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                let trimmed = value.trim();
                content_length = trimmed
                    .parse::<usize>()
                    .map_err(|_| format!("invalid Content-Length: {trimmed}"))?;
            }
        }
    }

    Ok(ParsedRequest {
        method: method.to_string(),
        target: parse_target(method, raw_target)?,
        version: version.to_string(),
        content_length,
    })
}

pub(crate) fn response_header(version: &str, status: &str, content_length: usize) -> Vec<u8> {
    format!(
        "{version} {status}\r\nContent-Length: {content_length}\r\nConnection: keep-alive\r\n\r\n"
    )
    .into_bytes()
}

pub(crate) fn connect_established(version: &str) -> Vec<u8> {
    format!("{version} 200 Connection Established\r\n\r\n").into_bytes()
}

pub(crate) fn bad_gateway(version: &str) -> Vec<u8> {
    response_header(version, "502 Bad Gateway", 0)
}

pub(crate) fn origin_form_header(original: &[u8], parsed: &ParsedRequest) -> Vec<u8> {
    let RequestTarget::Absolute { path, .. } = &parsed.target else {
        return original.to_vec();
    };

    let mut rewritten = Vec::new();
    rewritten
        .extend_from_slice(format!("{} {} {}\r\n", parsed.method, path, parsed.version).as_bytes());
    if let Some(line_end) = original.windows(2).position(|window| window == b"\r\n") {
        rewritten.extend_from_slice(&original[line_end + 2..]);
    }
    rewritten
}

fn parse_target(method: &str, raw_target: &str) -> Result<RequestTarget, String> {
    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = split_host_port(raw_target, 443)?;
        return Ok(RequestTarget::Connect { host, port });
    }

    if let Some(rest) = raw_target.strip_prefix("http://") {
        return parse_absolute_target("http", rest, 80);
    }
    if let Some(rest) = raw_target.strip_prefix("https://") {
        return parse_absolute_target("https", rest, 443);
    }

    Ok(RequestTarget::OriginForm {
        path: raw_target.to_string(),
    })
}

fn parse_absolute_target(
    scheme: &str,
    authority_and_path: &str,
    default_port: u16,
) -> Result<RequestTarget, String> {
    let path_start = authority_and_path.find(['/', '?']);
    let (authority, path) = match path_start {
        Some(index) if authority_and_path.as_bytes()[index] == b'?' => (
            &authority_and_path[..index],
            format!("/{}", &authority_and_path[index..]),
        ),
        Some(index) => (
            &authority_and_path[..index],
            authority_and_path[index..].to_string(),
        ),
        None => (authority_and_path, "/".to_string()),
    };
    let (host, port) = split_host_port(authority, default_port)?;

    Ok(RequestTarget::Absolute {
        scheme: scheme.to_string(),
        host,
        port,
        path,
    })
}

fn split_host_port(value: &str, default_port: u16) -> Result<(String, u16), String> {
    if value.is_empty() {
        return Err("missing host".to_string());
    }

    if let Some(rest) = value.strip_prefix('[') {
        let end = rest
            .find(']')
            .ok_or_else(|| format!("missing closing bracket in host: {value}"))?;
        let host = &rest[..end];
        let after_host = &rest[end + 1..];
        let port = if after_host.is_empty() {
            default_port
        } else if let Some(port) = after_host.strip_prefix(':') {
            parse_port(port)?
        } else {
            return Err(format!("invalid host port: {value}"));
        };
        if host.is_empty() {
            return Err("missing host".to_string());
        }
        return Ok((host.to_string(), port));
    }

    if let Some((host, port)) = value.rsplit_once(':') {
        if !host.contains(':') {
            if host.is_empty() {
                return Err("missing host".to_string());
            }
            return Ok((host.to_string(), parse_port(port)?));
        }
        return Err(format!("bracketed IPv6 is required in host: {value}"));
    }

    Ok((value.to_string(), default_port))
}

fn parse_port(value: &str) -> Result<u16, String> {
    if value.is_empty() {
        return Err("missing port".to_string());
    }
    value
        .parse::<u16>()
        .map_err(|_| format!("invalid port: {value}"))
}

#[cfg(test)]
mod tests {
    use super::{
        header_end, origin_form_header, parse_request_header, response_header, ParsedRequest,
        RequestTarget,
    };

    #[test]
    fn parses_connect_request() {
        let parsed = parse_request_header(
            b"CONNECT origin.example.com:443 HTTP/1.1\r\nHost: origin.example.com:443\r\n\r\n",
        )
        .unwrap();

        assert_eq!(
            parsed,
            ParsedRequest {
                method: "CONNECT".to_string(),
                target: RequestTarget::Connect {
                    host: "origin.example.com".to_string(),
                    port: 443,
                },
                version: "HTTP/1.1".to_string(),
                content_length: 0,
            }
        );
    }

    #[test]
    fn connect_without_port_defaults_to_443() {
        let parsed =
            parse_request_header(b"CONNECT example.com HTTP/1.1\r\nHost: example.com\r\n\r\n")
                .unwrap();

        assert_eq!(
            parsed.target,
            RequestTarget::Connect {
                host: "example.com".to_string(),
                port: 443,
            }
        );
    }

    #[test]
    fn parses_absolute_form_http_request() {
        let parsed = parse_request_header(
            b"GET http://127.0.0.1:18080/data?q=1 HTTP/1.1\r\nHost: 127.0.0.1:18080\r\nContent-Length: 3\r\n\r\n",
        )
        .unwrap();

        assert_eq!(
            parsed,
            ParsedRequest {
                method: "GET".to_string(),
                target: RequestTarget::Absolute {
                    scheme: "http".to_string(),
                    host: "127.0.0.1".to_string(),
                    port: 18080,
                    path: "/data?q=1".to_string(),
                },
                version: "HTTP/1.1".to_string(),
                content_length: 3,
            }
        );
    }

    #[test]
    fn parses_http_absolute_form_default_port() {
        let parsed = parse_request_header(
            b"GET http://example.com/path HTTP/1.1\r\nHost: example.com\r\n\r\n",
        )
        .unwrap();

        assert_eq!(
            parsed.target,
            RequestTarget::Absolute {
                scheme: "http".to_string(),
                host: "example.com".to_string(),
                port: 80,
                path: "/path".to_string(),
            }
        );
    }

    #[test]
    fn parses_https_absolute_form_default_port() {
        let parsed = parse_request_header(
            b"GET https://example.com/path HTTP/1.1\r\nHost: example.com\r\n\r\n",
        )
        .unwrap();

        assert_eq!(
            parsed.target,
            RequestTarget::Absolute {
                scheme: "https".to_string(),
                host: "example.com".to_string(),
                port: 443,
                path: "/path".to_string(),
            }
        );
    }

    #[test]
    fn parses_connect_ipv6_target() {
        let parsed =
            parse_request_header(b"CONNECT [::1]:443 HTTP/1.1\r\nHost: [::1]:443\r\n\r\n").unwrap();

        assert_eq!(
            parsed.target,
            RequestTarget::Connect {
                host: "::1".to_string(),
                port: 443,
            }
        );
    }

    #[test]
    fn parses_absolute_form_ipv6_target() {
        let parsed = parse_request_header(
            b"GET http://[::1]:18080/data HTTP/1.1\r\nHost: [::1]:18080\r\n\r\n",
        )
        .unwrap();

        assert_eq!(
            parsed.target,
            RequestTarget::Absolute {
                scheme: "http".to_string(),
                host: "::1".to_string(),
                port: 18080,
                path: "/data".to_string(),
            }
        );
    }

    #[test]
    fn rejects_unbracketed_ipv6_absolute_target() {
        let err = parse_request_header(b"GET http://::1:18080/data HTTP/1.1\r\nHost: ::1\r\n\r\n")
            .unwrap_err();

        assert!(err.contains("bracketed IPv6 is required"));
    }

    #[test]
    fn rejects_unbracketed_ipv6_connect_target() {
        let err =
            parse_request_header(b"CONNECT ::1:443 HTTP/1.1\r\nHost: ::1:443\r\n\r\n").unwrap_err();

        assert!(err.contains("bracketed IPv6 is required"));
    }

    #[test]
    fn origin_form_header_rewrites_first_line_and_preserves_headers() {
        let original = b"GET http://example.com/data HTTP/1.1\r\nHost: example.com\r\nUser-Agent: bench\r\n\r\n";
        let parsed = parse_request_header(original).unwrap();

        assert_eq!(
            origin_form_header(original, &parsed),
            b"GET /data HTTP/1.1\r\nHost: example.com\r\nUser-Agent: bench\r\n\r\n"
        );
    }

    #[test]
    fn response_header_includes_keep_alive() {
        assert_eq!(
            response_header("HTTP/1.1", "200 OK", 5),
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: keep-alive\r\n\r\n"
        );
    }

    #[test]
    fn finds_header_end() {
        assert_eq!(
            header_end(b"GET / HTTP/1.1\r\nHost: local\r\n\r\nbody"),
            Some(31)
        );
    }
}
