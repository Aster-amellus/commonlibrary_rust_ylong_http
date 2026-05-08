/*
 * Copyright (c) 2026 Huawei Device Co., Ltd.
 * Licensed under the Apache License, Version 2.0.
 */

#include <arpa/inet.h>
#include <errno.h>
#include <netdb.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <pthread.h>
#include <stdint.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <unistd.h>

#include <openssl/err.h>
#include <openssl/ssl.h>

#ifndef MSG_NOSIGNAL
#define MSG_NOSIGNAL 0
#endif

#define HEADER_CAP 65536
#define RELAY_BUF_SIZE 65536

typedef struct {
    int origin_port;
    int proxy_port;
    size_t response_size;
    int origin_tls;
    int proxy_tls;
    const char *cert_file;
    const char *key_file;
} Config;

typedef struct {
    int fd;
    SSL *ssl;
} Peer;

typedef struct {
    Peer peer;
    const Config *config;
    SSL_CTX *tls_ctx;
} Accepted;

typedef struct {
    int fd;
    const Config *config;
    SSL_CTX *tls_ctx;
    void *(*handler)(void *);
} AcceptLoop;

static char *g_body;

static const char *next_value(int *index, int argc, char **argv, const char *name)
{
    (*index)++;
    if (*index >= argc) {
        fprintf(stderr, "%s requires a value\n", name);
        exit(2);
    }
    return argv[*index];
}

static void usage(const char *program)
{
    fprintf(stderr,
            "usage: %s [--origin-port N] [--proxy-port N] [--response-size N] "
            "[--origin-tls] [--proxy-tls] [--cert-file PEM] [--key-file PEM]\n",
            program);
}

static Config parse_args(int argc, char **argv)
{
    Config config = {0};
    config.origin_port = 18080;
    config.proxy_port = 18443;
    config.response_size = 1024;

    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--origin-port") == 0) {
            config.origin_port = atoi(next_value(&i, argc, argv, "--origin-port"));
        } else if (strcmp(argv[i], "--proxy-port") == 0) {
            config.proxy_port = atoi(next_value(&i, argc, argv, "--proxy-port"));
        } else if (strcmp(argv[i], "--response-size") == 0) {
            config.response_size = strtoull(next_value(&i, argc, argv, "--response-size"), NULL, 10);
        } else if (strcmp(argv[i], "--origin-tls") == 0) {
            config.origin_tls = 1;
        } else if (strcmp(argv[i], "--proxy-tls") == 0) {
            config.proxy_tls = 1;
        } else if (strcmp(argv[i], "--cert-file") == 0) {
            config.cert_file = next_value(&i, argc, argv, "--cert-file");
        } else if (strcmp(argv[i], "--key-file") == 0) {
            config.key_file = next_value(&i, argc, argv, "--key-file");
        } else if (strcmp(argv[i], "--help") == 0 || strcmp(argv[i], "-h") == 0) {
            usage(argv[0]);
            exit(0);
        } else {
            fprintf(stderr, "unknown argument: %s\n", argv[i]);
            usage(argv[0]);
            exit(2);
        }
    }

    if (config.origin_port <= 0 || config.proxy_port <= 0 || config.response_size == 0) {
        usage(argv[0]);
        exit(2);
    }
    if ((config.origin_tls || config.proxy_tls) && (config.cert_file == NULL || config.key_file == NULL)) {
        fprintf(stderr, "--cert-file and --key-file are required when TLS is enabled\n");
        exit(2);
    }
    return config;
}

static void set_tcp_nodelay(int fd)
{
    int yes = 1;
    (void)setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &yes, sizeof(yes));
}

static int listen_on(int port)
{
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) {
        perror("socket");
        exit(2);
    }
    int yes = 1;
    (void)setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof(yes));

    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    addr.sin_port = htons((uint16_t)port);

    if (bind(fd, (struct sockaddr *)&addr, sizeof(addr)) != 0) {
        perror("bind");
        exit(2);
    }
    if (listen(fd, 1024) != 0) {
        perror("listen");
        exit(2);
    }
    return fd;
}

static SSL_CTX *create_tls_context(const Config *config)
{
    SSL_CTX *ctx = SSL_CTX_new(TLS_server_method());
    if (ctx == NULL) {
        ERR_print_errors_fp(stderr);
        exit(2);
    }
    if (SSL_CTX_use_certificate_chain_file(ctx, config->cert_file) != 1 ||
        SSL_CTX_use_PrivateKey_file(ctx, config->key_file, SSL_FILETYPE_PEM) != 1 ||
        SSL_CTX_check_private_key(ctx) != 1) {
        ERR_print_errors_fp(stderr);
        exit(2);
    }
    return ctx;
}

static int peer_accept_tls(Peer *peer, SSL_CTX *ctx)
{
    peer->ssl = NULL;
    if (ctx == NULL) {
        return 0;
    }
    peer->ssl = SSL_new(ctx);
    if (peer->ssl == NULL) {
        return -1;
    }
    SSL_set_fd(peer->ssl, peer->fd);
    if (SSL_accept(peer->ssl) != 1) {
        SSL_free(peer->ssl);
        peer->ssl = NULL;
        return -1;
    }
    return 0;
}

static ssize_t peer_read(Peer *peer, void *buf, size_t len)
{
    if (peer->ssl != NULL) {
        int ret = SSL_read(peer->ssl, buf, (int)(len > INT32_MAX ? INT32_MAX : len));
        return ret <= 0 ? -1 : ret;
    }
    return recv(peer->fd, buf, len, 0);
}

static ssize_t peer_write(Peer *peer, const void *data, size_t len)
{
    if (peer->ssl != NULL) {
        int ret = SSL_write(peer->ssl, data, (int)(len > INT32_MAX ? INT32_MAX : len));
        return ret <= 0 ? -1 : ret;
    }
    return send(peer->fd, data, len, MSG_NOSIGNAL);
}

static void peer_close(Peer *peer)
{
    if (peer->ssl != NULL) {
        SSL_shutdown(peer->ssl);
        SSL_free(peer->ssl);
        peer->ssl = NULL;
    }
    if (peer->fd >= 0) {
        close(peer->fd);
        peer->fd = -1;
    }
}

static ssize_t send_all(Peer *peer, const void *data, size_t len)
{
    const char *cursor = (const char *)data;
    size_t left = len;
    while (left != 0) {
        ssize_t sent = peer_write(peer, cursor, left);
        if (sent < 0) {
            if (errno == EINTR) {
                continue;
            }
            return -1;
        }
        if (sent == 0) {
            return -1;
        }
        cursor += sent;
        left -= (size_t)sent;
    }
    return (ssize_t)len;
}

static int has_header_end(const char *buf, size_t len, size_t *header_len)
{
    for (size_t i = 3; i < len; i++) {
        if (buf[i - 3] == '\r' && buf[i - 2] == '\n' && buf[i - 1] == '\r' && buf[i] == '\n') {
            *header_len = i + 1;
            return 1;
        }
    }
    return 0;
}

static ssize_t read_header(Peer *peer, char *buf, size_t *header_len)
{
    size_t len = 0;
    while (len < HEADER_CAP) {
        ssize_t n = peer_read(peer, buf + len, HEADER_CAP - len);
        if (n < 0) {
            if (errno == EINTR) {
                continue;
            }
            return -1;
        }
        if (n == 0) {
            return 0;
        }
        len += (size_t)n;
        if (has_header_end(buf, len, header_len)) {
            return (ssize_t)len;
        }
    }
    return -1;
}

static size_t content_length(const char *headers, size_t len)
{
    const char *end = headers + len;
    const char *p = headers;
    while (p < end) {
        const char *line = strstr(p, "\r\n");
        if (line == NULL || line > end) {
            return 0;
        }
        size_t line_len = (size_t)(line - p);
        if (line_len >= 15 && strncasecmp(p, "Content-Length:", 15) == 0) {
            return strtoull(p + 15, NULL, 10);
        }
        p = line + 2;
    }
    return 0;
}

static int drain_body(Peer *peer, const char *headers, size_t buffered_body_len, size_t header_len)
{
    size_t length = content_length(headers, header_len);
    if (buffered_body_len >= length) {
        return 0;
    }

    char buf[RELAY_BUF_SIZE];
    size_t left = length - buffered_body_len;
    while (left != 0) {
        ssize_t n = peer_read(peer, buf, left < sizeof(buf) ? left : sizeof(buf));
        if (n < 0) {
            if (errno == EINTR) {
                continue;
            }
            return -1;
        }
        if (n == 0) {
            return -1;
        }
        left -= (size_t)n;
    }
    return 0;
}

static int send_response(Peer *peer, const Config *config)
{
    char header[256];
    int len = snprintf(header, sizeof(header),
                       "HTTP/1.1 200 OK\r\n"
                       "Content-Length: %zu\r\n"
                       "Connection: keep-alive\r\n"
                       "\r\n",
                       config->response_size);
    if (len <= 0 || (size_t)len >= sizeof(header)) {
        return -1;
    }
    if (send_all(peer, header, (size_t)len) < 0) {
        return -1;
    }
    return send_all(peer, g_body, config->response_size) < 0 ? -1 : 0;
}

static int parse_connect_target(const char *target, char *host, size_t host_len, char *port,
                                size_t port_len)
{
    if (target[0] == '[') {
        const char *close = strchr(target, ']');
        if (close == NULL || close[1] != ':') {
            return -1;
        }
        size_t len = (size_t)(close - target - 1);
        if (len >= host_len) {
            return -1;
        }
        memcpy(host, target + 1, len);
        host[len] = '\0';
        snprintf(port, port_len, "%s", close + 2);
        return 0;
    }

    const char *colon = strrchr(target, ':');
    if (colon == NULL) {
        return -1;
    }
    size_t len = (size_t)(colon - target);
    if (len >= host_len) {
        return -1;
    }
    memcpy(host, target, len);
    host[len] = '\0';
    snprintf(port, port_len, "%s", colon + 1);
    return 0;
}

static int connect_target(const char *target)
{
    char host[256];
    char port[16];
    if (parse_connect_target(target, host, sizeof(host), port, sizeof(port)) != 0) {
        return -1;
    }

    struct addrinfo hints;
    struct addrinfo *res = NULL;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    if (getaddrinfo(host, port, &hints, &res) != 0) {
        return -1;
    }

    int fd = -1;
    for (struct addrinfo *it = res; it != NULL; it = it->ai_next) {
        fd = socket(it->ai_family, it->ai_socktype, it->ai_protocol);
        if (fd < 0) {
            continue;
        }
        if (connect(fd, it->ai_addr, it->ai_addrlen) == 0) {
            set_tcp_nodelay(fd);
            break;
        }
        close(fd);
        fd = -1;
    }
    freeaddrinfo(res);
    return fd;
}

static void relay(Peer *left, Peer *right)
{
    char buf[RELAY_BUF_SIZE];
    while (1) {
        fd_set readfds;
        FD_ZERO(&readfds);
        FD_SET(left->fd, &readfds);
        FD_SET(right->fd, &readfds);
        int max_fd = left->fd > right->fd ? left->fd : right->fd;
        int ready = select(max_fd + 1, &readfds, NULL, NULL, NULL);
        if (ready < 0) {
            if (errno == EINTR) {
                continue;
            }
            return;
        }
        if (FD_ISSET(left->fd, &readfds)) {
            ssize_t n = peer_read(left, buf, sizeof(buf));
            if (n <= 0 || send_all(right, buf, (size_t)n) < 0) {
                return;
            }
        }
        if (FD_ISSET(right->fd, &readfds)) {
            ssize_t n = peer_read(right, buf, sizeof(buf));
            if (n <= 0 || send_all(left, buf, (size_t)n) < 0) {
                return;
            }
        }
    }
}

static void *origin_client(void *arg)
{
    Accepted *accepted = (Accepted *)arg;
    Peer peer = accepted->peer;
    const Config *config = accepted->config;
    SSL_CTX *tls_ctx = accepted->tls_ctx;
    free(accepted);
    set_tcp_nodelay(peer.fd);
    if (peer_accept_tls(&peer, tls_ctx) != 0) {
        peer_close(&peer);
        return NULL;
    }

    char buf[HEADER_CAP];
    while (1) {
        size_t header_len = 0;
        ssize_t total = read_header(&peer, buf, &header_len);
        if (total <= 0) {
            break;
        }
        size_t rest_len = (size_t)total - header_len;
        if (drain_body(&peer, buf, rest_len, header_len) != 0) {
            break;
        }
        if (send_response(&peer, config) != 0) {
            break;
        }
    }
    peer_close(&peer);
    return NULL;
}

static void *proxy_client(void *arg)
{
    Accepted *accepted = (Accepted *)arg;
    Peer peer = accepted->peer;
    const Config *config = accepted->config;
    SSL_CTX *tls_ctx = accepted->tls_ctx;
    free(accepted);
    set_tcp_nodelay(peer.fd);
    if (peer_accept_tls(&peer, tls_ctx) != 0) {
        peer_close(&peer);
        return NULL;
    }

    char buf[HEADER_CAP + 1];
    while (1) {
        size_t header_len = 0;
        ssize_t total = read_header(&peer, buf, &header_len);
        if (total <= 0) {
            break;
        }
        buf[header_len < HEADER_CAP ? header_len : HEADER_CAP] = '\0';

        char method[16];
        char target[512];
        char version[16];
        if (sscanf(buf, "%15s %511s %15s", method, target, version) != 3) {
            break;
        }

        size_t rest_len = (size_t)total - header_len;
        if (strcasecmp(method, "CONNECT") == 0) {
            int upstream = connect_target(target);
            if (upstream < 0) {
                const char *response = "HTTP/1.1 502 Bad Gateway\r\n\r\n";
                (void)send_all(&peer, response, strlen(response));
                break;
            }
            Peer upstream_peer = {.fd = upstream, .ssl = NULL};
            const char *response = "HTTP/1.1 200 Connection Established\r\n\r\n";
            if (send_all(&peer, response, strlen(response)) < 0) {
                peer_close(&upstream_peer);
                break;
            }
            if (rest_len != 0 && send_all(&upstream_peer, buf + header_len, rest_len) < 0) {
                peer_close(&upstream_peer);
                break;
            }
            relay(&peer, &upstream_peer);
            peer_close(&upstream_peer);
            break;
        }

        if (drain_body(&peer, buf, rest_len, header_len) != 0 ||
            send_response(&peer, config) != 0) {
            break;
        }
    }
    peer_close(&peer);
    return NULL;
}

static void spawn_client(int fd, const Config *config, SSL_CTX *tls_ctx, void *(*handler)(void *))
{
    Accepted *accepted = malloc(sizeof(*accepted));
    if (accepted == NULL) {
        close(fd);
        return;
    }
    accepted->peer.fd = fd;
    accepted->peer.ssl = NULL;
    accepted->config = config;
    accepted->tls_ctx = tls_ctx;

    pthread_t thread;
    if (pthread_create(&thread, NULL, handler, accepted) != 0) {
        close(fd);
        free(accepted);
        return;
    }
    pthread_detach(thread);
}

static void *accept_loop(void *arg)
{
    AcceptLoop *loop = (AcceptLoop *)arg;
    int listener = loop->fd;
    const Config *config = loop->config;
    SSL_CTX *tls_ctx = loop->tls_ctx;
    void *(*handler)(void *) = loop->handler;
    free(loop);

    while (1) {
        int fd = accept(listener, NULL, NULL);
        if (fd < 0) {
            if (errno == EINTR) {
                continue;
            }
            perror("accept");
            return NULL;
        }
        spawn_client(fd, config, tls_ctx, handler);
    }
}

int main(int argc, char **argv)
{
    signal(SIGPIPE, SIG_IGN);
    Config config = parse_args(argc, argv);
    if (config.origin_tls || config.proxy_tls) {
        SSL_library_init();
        SSL_load_error_strings();
        OpenSSL_add_ssl_algorithms();
    }
    SSL_CTX *origin_tls_ctx = config.origin_tls ? create_tls_context(&config) : NULL;
    SSL_CTX *proxy_tls_ctx = config.proxy_tls ? create_tls_context(&config) : NULL;

    g_body = malloc(config.response_size);
    if (g_body == NULL) {
        fprintf(stderr, "response body allocation failed\n");
        return 2;
    }
    memset(g_body, 'x', config.response_size);

    int origin = listen_on(config.origin_port);
    int proxy = listen_on(config.proxy_port);

    AcceptLoop *origin_arg = malloc(sizeof(*origin_arg));
    AcceptLoop *proxy_arg = malloc(sizeof(*proxy_arg));
    if (origin_arg == NULL || proxy_arg == NULL) {
        return 2;
    }
    origin_arg->fd = origin;
    origin_arg->config = &config;
    origin_arg->tls_ctx = origin_tls_ctx;
    origin_arg->handler = origin_client;
    proxy_arg->fd = proxy;
    proxy_arg->config = &config;
    proxy_arg->tls_ctx = proxy_tls_ctx;
    proxy_arg->handler = proxy_client;

    pthread_t origin_thread;
    pthread_t proxy_thread;
    if (pthread_create(&origin_thread, NULL, accept_loop, origin_arg) != 0 ||
        pthread_create(&proxy_thread, NULL, accept_loop, proxy_arg) != 0) {
        perror("pthread_create");
        return 2;
    }

    printf("{\"origin\":\"%s://127.0.0.1:%d/\",\"proxy\":\"%s://localhost:%d\"}\n",
           config.origin_tls ? "https" : "http", config.origin_port,
           config.proxy_tls ? "https" : "http", config.proxy_port);
    fflush(stdout);

    pthread_join(origin_thread, NULL);
    pthread_join(proxy_thread, NULL);
    return 0;
}
