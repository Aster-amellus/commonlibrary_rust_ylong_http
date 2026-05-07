#!/usr/bin/env bash
set -euo pipefail

OUT_DIR=${1:-target/https_proxy_bench/certs}
mkdir -p "$OUT_DIR"

cat >"$OUT_DIR/localhost.ext" <<'EOF'
subjectAltName = DNS:localhost,IP:127.0.0.1
extendedKeyUsage = serverAuth
EOF

openssl req -x509 -newkey rsa:2048 -days 1 -nodes \
    -subj "/CN=ylong-bench-ca" \
    -keyout "$OUT_DIR/ca.key" \
    -out "$OUT_DIR/ca.pem"

openssl req -newkey rsa:2048 -nodes \
    -subj "/CN=localhost" \
    -keyout "$OUT_DIR/server.key" \
    -out "$OUT_DIR/server.csr"

openssl x509 -req -days 1 \
    -in "$OUT_DIR/server.csr" \
    -CA "$OUT_DIR/ca.pem" \
    -CAkey "$OUT_DIR/ca.key" \
    -CAcreateserial \
    -extfile "$OUT_DIR/localhost.ext" \
    -out "$OUT_DIR/server.pem"

openssl req -newkey rsa:2048 -nodes \
    -subj "/CN=ylong-bench-client" \
    -keyout "$OUT_DIR/client.key" \
    -out "$OUT_DIR/client.csr"

openssl x509 -req -days 1 \
    -in "$OUT_DIR/client.csr" \
    -CA "$OUT_DIR/ca.pem" \
    -CAkey "$OUT_DIR/ca.key" \
    -CAcreateserial \
    -out "$OUT_DIR/client.pem"

echo "$OUT_DIR"
