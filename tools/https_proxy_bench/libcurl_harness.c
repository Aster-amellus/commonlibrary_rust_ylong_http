/*
 * Copyright (c) 2026 Huawei Device Co., Ltd.
 * Licensed under the Apache License, Version 2.0.
 */

#include <curl/curl.h>
#include <errno.h>
#include <limits.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

typedef struct {
    const char *url;
    const char *proxy;
    const char *proxy_ca_file;
    const char *proxy_client_cert;
    const char *proxy_client_key;
    const char *origin_ca_file;
    const char *proxy_user_pass;
    const char *tls13_ciphers;
    const char *tls_groups;
    const char *http_version;
    long proxy_type;
    long http_version_opt;
    long insecure_proxy;
    long insecure_origin;
    long libcurl_pipewait;
    long libcurl_max_host_connections;
    long libcurl_max_total_connections;
    long libcurl_max_concurrent_streams;
    int share_connections;
    const char *libcurl_mode;
    size_t requests;
    size_t warmup_requests;
    uint64_t duration_seconds;
    int duration_seconds_set;
    size_t concurrency;
    size_t read_buffer_size;
} Config;

typedef struct {
    const Config *config;
    pthread_barrier_t *ready_barrier;
    pthread_barrier_t *start_barrier;
    size_t count;
    size_t warmup_count;
    uint64_t *latencies_us;
    size_t latency_capacity;
    CURLSH *share;
    uint64_t bytes;
    size_t completed;
    size_t errors;
} Worker;

typedef struct {
    const Config *config;
    CURL *curl;
    uint64_t bytes_current;
    uint64_t started_us;
    int active;
} MultiTransfer;

typedef struct {
    const Config *config;
    uint64_t *latencies_us;
    size_t latency_capacity;
    uint64_t bytes;
    size_t completed;
    size_t errors;
} MultiResult;

typedef struct {
    MultiResult *result;
    size_t remaining;
    uint64_t deadline_us;
    int duration_mode;
    int record;
} MultiPhase;

static size_t write_body(char *ptr, size_t size, size_t nmemb, void *userdata)
{
    Worker *worker = (Worker *)userdata;
    size_t total = size * nmemb;
    (void)ptr;
    worker->bytes += (uint64_t)total;
    return total;
}

static size_t write_body_multi(char *ptr, size_t size, size_t nmemb, void *userdata)
{
    MultiTransfer *transfer = (MultiTransfer *)userdata;
    size_t total = size * nmemb;
    (void)ptr;
    transfer->bytes_current += (uint64_t)total;
    return total;
}

static uint64_t now_us(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000ULL + (uint64_t)ts.tv_nsec / 1000ULL;
}

static void curl_lock(CURL *handle, curl_lock_data data, curl_lock_access access, void *userptr)
{
    (void)handle;
    (void)data;
    (void)access;
    pthread_mutex_lock((pthread_mutex_t *)userptr);
}

static void curl_unlock(CURL *handle, curl_lock_data data, void *userptr)
{
    (void)handle;
    (void)data;
    pthread_mutex_unlock((pthread_mutex_t *)userptr);
}

static int cmp_u64(const void *a, const void *b)
{
    uint64_t left = *(const uint64_t *)a;
    uint64_t right = *(const uint64_t *)b;
    return (left > right) - (left < right);
}

static uint64_t percentile(const uint64_t *values, size_t len, size_t pct)
{
    if (len == 0) {
        return 0;
    }
    return values[((len - 1) * pct) / 100];
}

static uint64_t percentile_permille(const uint64_t *values, size_t len, size_t permille)
{
    if (len == 0) {
        return 0;
    }
    return values[((len - 1) * permille) / 1000];
}

static int record_latency(Worker *worker, uint64_t latency_us)
{
    if (worker->completed == worker->latency_capacity) {
        if (worker->latency_capacity > SIZE_MAX / 2 / sizeof(uint64_t)) {
            return 0;
        }
        size_t new_capacity = worker->latency_capacity == 0 ? 1024 : worker->latency_capacity * 2;
        uint64_t *new_latencies =
            realloc(worker->latencies_us, new_capacity * sizeof(uint64_t));
        if (new_latencies == NULL) {
            return 0;
        }
        worker->latencies_us = new_latencies;
        worker->latency_capacity = new_capacity;
    }
    worker->latencies_us[worker->completed] = latency_us;
    worker->completed++;
    return 1;
}

static int record_multi_latency(MultiResult *result, uint64_t latency_us)
{
    if (result->completed == result->latency_capacity) {
        if (result->latency_capacity > SIZE_MAX / 2 / sizeof(uint64_t)) {
            return 0;
        }
        size_t new_capacity = result->latency_capacity == 0 ? 1024 : result->latency_capacity * 2;
        uint64_t *new_latencies =
            realloc(result->latencies_us, new_capacity * sizeof(uint64_t));
        if (new_latencies == NULL) {
            return 0;
        }
        result->latencies_us = new_latencies;
        result->latency_capacity = new_capacity;
    }
    result->latencies_us[result->completed] = latency_us;
    result->completed++;
    return 1;
}

static void set_curl_options(CURL *curl, const Config *config, void *write_data,
                             size_t (*write_callback)(char *, size_t, size_t, void *),
                             CURLSH *share)
{
    curl_easy_setopt(curl, CURLOPT_URL, config->url);
    curl_easy_setopt(curl, CURLOPT_PROXY, config->proxy);
    curl_easy_setopt(curl, CURLOPT_PROXYTYPE, config->proxy_type);
    curl_easy_setopt(curl, CURLOPT_HTTP_VERSION, config->http_version_opt);
    curl_easy_setopt(curl, CURLOPT_NOSIGNAL, 1L);
    curl_easy_setopt(curl, CURLOPT_WRITEFUNCTION, write_callback);
    curl_easy_setopt(curl, CURLOPT_WRITEDATA, write_data);
    curl_easy_setopt(curl, CURLOPT_BUFFERSIZE, (long)config->read_buffer_size);
    if (share != NULL) {
        curl_easy_setopt(curl, CURLOPT_SHARE, share);
    }
    if (config->tls13_ciphers != NULL) {
        curl_easy_setopt(curl, CURLOPT_TLS13_CIPHERS, config->tls13_ciphers);
        curl_easy_setopt(curl, CURLOPT_PROXY_TLS13_CIPHERS, config->tls13_ciphers);
    }
    if (config->tls_groups != NULL) {
        curl_easy_setopt(curl, CURLOPT_SSL_EC_CURVES, config->tls_groups);
    }
    if (config->libcurl_pipewait) {
        curl_easy_setopt(curl, CURLOPT_PIPEWAIT, 1L);
    }

    if (config->proxy_ca_file != NULL) {
        curl_easy_setopt(curl, CURLOPT_PROXY_CAINFO, config->proxy_ca_file);
    }
    if (config->proxy_client_cert != NULL) {
        curl_easy_setopt(curl, CURLOPT_PROXY_SSLCERT, config->proxy_client_cert);
    }
    if (config->proxy_client_key != NULL) {
        curl_easy_setopt(curl, CURLOPT_PROXY_SSLKEY, config->proxy_client_key);
    }
    if (config->origin_ca_file != NULL) {
        curl_easy_setopt(curl, CURLOPT_CAINFO, config->origin_ca_file);
    }
    if (config->proxy_user_pass != NULL) {
        curl_easy_setopt(curl, CURLOPT_PROXYUSERPWD, config->proxy_user_pass);
    }
    if (config->insecure_proxy) {
        curl_easy_setopt(curl, CURLOPT_PROXY_SSL_VERIFYPEER, 0L);
        curl_easy_setopt(curl, CURLOPT_PROXY_SSL_VERIFYHOST, 0L);
    }
    if (config->insecure_origin) {
        curl_easy_setopt(curl, CURLOPT_SSL_VERIFYPEER, 0L);
        curl_easy_setopt(curl, CURLOPT_SSL_VERIFYHOST, 0L);
    }
}

static void set_common_options(CURL *curl, Worker *worker)
{
    set_curl_options(curl, worker->config, worker, write_body, worker->share);
}

static void *run_worker(void *arg)
{
    Worker *worker = (Worker *)arg;
    CURL *curl = curl_easy_init();
    if (curl == NULL) {
        worker->errors +=
            worker->warmup_count + (worker->config->duration_seconds > 0 ? 1 : worker->count);
        pthread_barrier_wait(worker->ready_barrier);
        pthread_barrier_wait(worker->start_barrier);
        return NULL;
    }

    set_common_options(curl, worker);

    for (size_t i = 0; i < worker->warmup_count; i++) {
        if (curl_easy_perform(curl) != CURLE_OK) {
            worker->errors++;
        }
    }
    worker->bytes = 0;

    pthread_barrier_wait(worker->ready_barrier);
    pthread_barrier_wait(worker->start_barrier);

    if (worker->config->duration_seconds > 0) {
        uint64_t deadline = now_us() + worker->config->duration_seconds * 1000000ULL;
        while (now_us() < deadline) {
            uint64_t started = now_us();
            if (curl_easy_perform(curl) == CURLE_OK) {
                if (!record_latency(worker, now_us() - started)) {
                    worker->errors++;
                    break;
                }
            } else {
                worker->errors++;
            }
        }
        curl_easy_cleanup(curl);
        return NULL;
    }

    for (size_t i = 0; i < worker->count; i++) {
        uint64_t started = now_us();
        if (curl_easy_perform(curl) == CURLE_OK) {
            if (!record_latency(worker, now_us() - started)) {
                worker->errors++;
                break;
            }
        } else {
            worker->errors++;
        }
    }

    curl_easy_cleanup(curl);
    return NULL;
}

static const char *next_value(int *index, int argc, char **argv, const char *name)
{
    (*index)++;
    if (*index >= argc) {
        fprintf(stderr, "%s requires a value\n", name);
        exit(2);
    }
    return argv[*index];
}

static long next_positive_long(int *index, int argc, char **argv, const char *name)
{
    const char *value = next_value(index, argc, argv, name);
    char *end = NULL;
    errno = 0;
    long parsed = strtol(value, &end, 10);
    if (errno != 0 || end == value || *end != '\0' || parsed <= 0) {
        fprintf(stderr, "%s requires a positive integer\n", name);
        exit(2);
    }
    return parsed;
}

static void usage(const char *program)
{
    fprintf(stderr,
            "usage: %s --url URL --proxy http[s]://PROXY[:PORT] [--requests N] "
            "[--warmup-requests N] [--duration-seconds N] [--concurrency N] "
            "[--read-buffer-size N] "
            "[--proxy-ca-file PEM] [--proxy-client-cert PEM] [--proxy-client-key PEM] "
            "[--origin-ca-file PEM] [--insecure-proxy] [--insecure-origin] "
            "[--proxy-user-pass user:pass] [--http-version h1|h2|negotiate] "
            "[--tls13-ciphers LIST] [--tls-groups LIST] "
            "[--share-connections] [--libcurl-mode easy-threads|multi] "
            "[--libcurl-pipewait] [--libcurl-max-host-connections N] "
            "[--libcurl-max-total-connections N] "
            "[--libcurl-max-concurrent-streams N]\n",
            program);
}

static Config parse_args(int argc, char **argv)
{
    Config config;
    memset(&config, 0, sizeof(config));
    config.requests = 1000;
    config.concurrency = 16;
    config.read_buffer_size = 64 * 1024;
    config.http_version = "h1";
    config.http_version_opt = CURL_HTTP_VERSION_1_1;
    config.libcurl_mode = "easy-threads";

    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--url") == 0) {
            config.url = next_value(&i, argc, argv, "--url");
        } else if (strcmp(argv[i], "--proxy") == 0) {
            config.proxy = next_value(&i, argc, argv, "--proxy");
        } else if (strcmp(argv[i], "--requests") == 0) {
            config.requests = strtoull(next_value(&i, argc, argv, "--requests"), NULL, 10);
        } else if (strcmp(argv[i], "--warmup-requests") == 0) {
            config.warmup_requests =
                strtoull(next_value(&i, argc, argv, "--warmup-requests"), NULL, 10);
        } else if (strcmp(argv[i], "--duration-seconds") == 0) {
            config.duration_seconds =
                strtoull(next_value(&i, argc, argv, "--duration-seconds"), NULL, 10);
            config.duration_seconds_set = 1;
        } else if (strcmp(argv[i], "--concurrency") == 0) {
            config.concurrency = strtoull(next_value(&i, argc, argv, "--concurrency"), NULL, 10);
        } else if (strcmp(argv[i], "--read-buffer-size") == 0) {
            config.read_buffer_size =
                strtoull(next_value(&i, argc, argv, "--read-buffer-size"), NULL, 10);
        } else if (strcmp(argv[i], "--proxy-ca-file") == 0) {
            config.proxy_ca_file = next_value(&i, argc, argv, "--proxy-ca-file");
        } else if (strcmp(argv[i], "--proxy-client-cert") == 0) {
            config.proxy_client_cert = next_value(&i, argc, argv, "--proxy-client-cert");
        } else if (strcmp(argv[i], "--proxy-client-key") == 0) {
            config.proxy_client_key = next_value(&i, argc, argv, "--proxy-client-key");
        } else if (strcmp(argv[i], "--origin-ca-file") == 0) {
            config.origin_ca_file = next_value(&i, argc, argv, "--origin-ca-file");
        } else if (strcmp(argv[i], "--proxy-user-pass") == 0) {
            config.proxy_user_pass = next_value(&i, argc, argv, "--proxy-user-pass");
        } else if (strcmp(argv[i], "--http-version") == 0) {
            const char *value = next_value(&i, argc, argv, "--http-version");
            if (strcmp(value, "h1") == 0 || strcmp(value, "http1") == 0 ||
                strcmp(value, "http/1.1") == 0) {
                config.http_version = "h1";
                config.http_version_opt = CURL_HTTP_VERSION_1_1;
            } else if (strcmp(value, "h2") == 0 || strcmp(value, "http2") == 0 ||
                       strcmp(value, "http/2") == 0) {
                config.http_version = "h2";
                config.http_version_opt = CURL_HTTP_VERSION_2TLS;
            } else if (strcmp(value, "negotiate") == 0 || strcmp(value, "alpn") == 0) {
                config.http_version = "negotiate";
                config.http_version_opt = CURL_HTTP_VERSION_NONE;
            } else {
                fprintf(stderr, "invalid --http-version: %s\n", value);
                exit(2);
            }
        } else if (strcmp(argv[i], "--tls13-ciphers") == 0) {
            config.tls13_ciphers = next_value(&i, argc, argv, "--tls13-ciphers");
        } else if (strcmp(argv[i], "--tls-groups") == 0) {
            config.tls_groups = next_value(&i, argc, argv, "--tls-groups");
        } else if (strcmp(argv[i], "--share-connections") == 0) {
            config.share_connections = 1;
        } else if (strcmp(argv[i], "--libcurl-mode") == 0) {
            const char *value = next_value(&i, argc, argv, "--libcurl-mode");
            if (strcmp(value, "easy-threads") != 0 && strcmp(value, "multi") != 0) {
                fprintf(stderr, "invalid --libcurl-mode: %s\n", value);
                exit(2);
            }
            config.libcurl_mode = value;
        } else if (strcmp(argv[i], "--libcurl-pipewait") == 0) {
            config.libcurl_pipewait = 1L;
        } else if (strcmp(argv[i], "--libcurl-max-host-connections") == 0) {
            config.libcurl_max_host_connections =
                next_positive_long(&i, argc, argv, "--libcurl-max-host-connections");
        } else if (strcmp(argv[i], "--libcurl-max-total-connections") == 0) {
            config.libcurl_max_total_connections =
                next_positive_long(&i, argc, argv, "--libcurl-max-total-connections");
        } else if (strcmp(argv[i], "--libcurl-max-concurrent-streams") == 0) {
            config.libcurl_max_concurrent_streams =
                next_positive_long(&i, argc, argv, "--libcurl-max-concurrent-streams");
        } else if (strcmp(argv[i], "--insecure-proxy") == 0) {
            config.insecure_proxy = 1L;
        } else if (strcmp(argv[i], "--insecure-origin") == 0) {
            config.insecure_origin = 1L;
        } else if (strcmp(argv[i], "--help") == 0 || strcmp(argv[i], "-h") == 0) {
            usage(argv[0]);
            exit(0);
        } else {
            fprintf(stderr, "unknown argument: %s\n", argv[i]);
            usage(argv[0]);
            exit(2);
        }
    }

    if (config.url == NULL || config.proxy == NULL || config.concurrency == 0 ||
        config.read_buffer_size == 0 || (config.requests == 0 && config.duration_seconds == 0)) {
        usage(argv[0]);
        exit(2);
    }
    if (config.duration_seconds_set && config.duration_seconds == 0) {
        fprintf(stderr, "--duration-seconds must be > 0\n");
        exit(2);
    }
    if (strncmp(config.proxy, "https://", strlen("https://")) == 0) {
        config.proxy_type = CURLPROXY_HTTPS;
    } else if (strncmp(config.proxy, "http://", strlen("http://")) == 0) {
        config.proxy_type = CURLPROXY_HTTP;
    } else {
        fprintf(stderr, "--proxy must start with http:// or https://\n");
        exit(2);
    }
    if ((config.proxy_client_cert == NULL) != (config.proxy_client_key == NULL)) {
        fprintf(stderr, "--proxy-client-cert and --proxy-client-key must be set together\n");
        exit(2);
    }
    return config;
}

static int multi_should_start(const MultiPhase *phase)
{
    if (phase->duration_mode) {
        return now_us() < phase->deadline_us;
    }
    return phase->remaining > 0;
}

static int multi_start_transfer(CURLM *multi, MultiTransfer *transfer, MultiPhase *phase)
{
    if (!multi_should_start(phase)) {
        return 0;
    }
    if (!phase->duration_mode) {
        phase->remaining--;
    }
    transfer->bytes_current = 0;
    transfer->started_us = now_us();
    transfer->active = 1;
    CURLMcode result = curl_multi_add_handle(multi, transfer->curl);
    if (result != CURLM_OK) {
        fprintf(stderr, "curl_multi_add_handle failed: %s\n", curl_multi_strerror(result));
        transfer->active = 0;
        phase->result->errors++;
        return -1;
    }
    return 1;
}

static int multi_complete_transfer(CURLM *multi, CURLMsg *message, MultiPhase *phase)
{
    char *private_data = NULL;
    CURLcode info_result =
        curl_easy_getinfo(message->easy_handle, CURLINFO_PRIVATE, &private_data);
    MultiTransfer *transfer = (MultiTransfer *)private_data;
    curl_multi_remove_handle(multi, message->easy_handle);
    if (info_result != CURLE_OK || transfer == NULL) {
        phase->result->errors++;
        return -1;
    }

    transfer->active = 0;
    if (message->data.result == CURLE_OK) {
        if (phase->record) {
            phase->result->bytes += transfer->bytes_current;
            if (!record_multi_latency(phase->result, now_us() - transfer->started_us)) {
                phase->result->errors++;
                return -1;
            }
        }
    } else {
        phase->result->errors++;
    }

    return multi_start_transfer(multi, transfer, phase);
}

static int run_multi_phase(CURLM *multi, MultiTransfer *transfers, size_t transfer_count,
                           MultiPhase *phase)
{
    int still_running = 0;
    int active = 0;

    for (size_t i = 0; i < transfer_count; i++) {
        int started = multi_start_transfer(multi, &transfers[i], phase);
        if (started < 0) {
            return 0;
        }
        active += started;
        if (!multi_should_start(phase)) {
            break;
        }
    }

    while (active > 0) {
        CURLMcode multi_result = curl_multi_perform(multi, &still_running);
        if (multi_result != CURLM_OK) {
            fprintf(stderr, "curl_multi_perform failed: %s\n", curl_multi_strerror(multi_result));
            return 0;
        }

        int messages_left = 0;
        CURLMsg *message = NULL;
        while ((message = curl_multi_info_read(multi, &messages_left)) != NULL) {
            if (message->msg != CURLMSG_DONE) {
                continue;
            }
            int started = multi_complete_transfer(multi, message, phase);
            if (started < 0) {
                return 0;
            }
            active += started - 1;
        }

        if (active == 0) {
            break;
        }

        multi_result = curl_multi_poll(multi, NULL, 0, 1000, NULL);
        if (multi_result != CURLM_OK) {
            fprintf(stderr, "curl_multi_poll failed: %s\n", curl_multi_strerror(multi_result));
            return 0;
        }
    }

    return 1;
}

static void cleanup_multi_transfers(MultiTransfer *transfers, size_t count)
{
    for (size_t i = 0; i < count; i++) {
        if (transfers[i].curl != NULL) {
            curl_easy_cleanup(transfers[i].curl);
        }
    }
}

static int run_multi_benchmark(const Config *config)
{
    if (config->share_connections) {
        fprintf(stderr, "--share-connections is only supported with easy-threads mode\n");
        return 2;
    }

    CURLM *multi = curl_multi_init();
    MultiTransfer *transfers = calloc(config->concurrency, sizeof(MultiTransfer));
    MultiResult result;
    memset(&result, 0, sizeof(result));
    result.config = config;
    result.latency_capacity =
        config->duration_seconds > 0 || config->requests == 0 ? 1024 : config->requests;
    result.latencies_us = calloc(result.latency_capacity, sizeof(uint64_t));

    if (multi == NULL || transfers == NULL || result.latencies_us == NULL) {
        fprintf(stderr, "allocation failed\n");
        curl_multi_cleanup(multi);
        free(transfers);
        free(result.latencies_us);
        return 2;
    }
    CURLMcode multi_result = curl_multi_setopt(multi, CURLMOPT_PIPELINING, CURLPIPE_MULTIPLEX);
    if (multi_result == CURLM_OK && config->libcurl_max_host_connections > 0) {
        multi_result = curl_multi_setopt(multi, CURLMOPT_MAX_HOST_CONNECTIONS,
                                         config->libcurl_max_host_connections);
    }
    if (multi_result == CURLM_OK && config->libcurl_max_total_connections > 0) {
        multi_result = curl_multi_setopt(multi, CURLMOPT_MAX_TOTAL_CONNECTIONS,
                                         config->libcurl_max_total_connections);
    }
    if (multi_result == CURLM_OK && config->libcurl_max_concurrent_streams > 0) {
        multi_result = curl_multi_setopt(multi, CURLMOPT_MAX_CONCURRENT_STREAMS,
                                         config->libcurl_max_concurrent_streams);
    }
    if (multi_result != CURLM_OK) {
        fprintf(stderr, "curl_multi_setopt failed: %s\n", curl_multi_strerror(multi_result));
        cleanup_multi_transfers(transfers, config->concurrency);
        curl_multi_cleanup(multi);
        free(transfers);
        free(result.latencies_us);
        return 2;
    }

    for (size_t i = 0; i < config->concurrency; i++) {
        transfers[i].config = config;
        transfers[i].curl = curl_easy_init();
        if (transfers[i].curl == NULL) {
            fprintf(stderr, "curl_easy_init failed\n");
            cleanup_multi_transfers(transfers, config->concurrency);
            curl_multi_cleanup(multi);
            free(transfers);
            free(result.latencies_us);
            return 2;
        }
        set_curl_options(transfers[i].curl, config, &transfers[i], write_body_multi, NULL);
        curl_easy_setopt(transfers[i].curl, CURLOPT_PRIVATE, &transfers[i]);
    }

    MultiPhase warmup;
    memset(&warmup, 0, sizeof(warmup));
    warmup.result = &result;
    warmup.remaining = config->warmup_requests;
    warmup.record = 0;
    if (!run_multi_phase(multi, transfers, config->concurrency, &warmup)) {
        cleanup_multi_transfers(transfers, config->concurrency);
        curl_multi_cleanup(multi);
        free(transfers);
        free(result.latencies_us);
        return 1;
    }

    MultiPhase measured;
    memset(&measured, 0, sizeof(measured));
    measured.result = &result;
    measured.remaining = config->requests;
    measured.record = 1;
    if (config->duration_seconds > 0) {
        measured.duration_mode = 1;
        measured.deadline_us = now_us() + config->duration_seconds * 1000000ULL;
    }

    uint64_t started = now_us();
    if (!run_multi_phase(multi, transfers, config->concurrency, &measured)) {
        cleanup_multi_transfers(transfers, config->concurrency);
        curl_multi_cleanup(multi);
        free(transfers);
        free(result.latencies_us);
        return 1;
    }
    uint64_t elapsed_us = now_us() - started;

    if (result.completed > 0) {
        qsort(result.latencies_us, result.completed, sizeof(uint64_t), cmp_u64);
    }

    char duration_seconds[32];
    if (config->duration_seconds > 0) {
        snprintf(duration_seconds, sizeof(duration_seconds), "%llu",
                 (unsigned long long)config->duration_seconds);
    } else {
        snprintf(duration_seconds, sizeof(duration_seconds), "null");
    }

    double rps =
        elapsed_us == 0 ? 0.0 : (double)result.completed * 1000000.0 / (double)elapsed_us;
    printf("{\"client\":\"libcurl\",\"url\":\"%s\",\"proxy\":\"%s\","
           "\"requests\":%zu,\"warmup_requests\":%zu,\"duration_seconds\":%s,\"completed\":%zu,"
           "\"errors\":%zu,\"concurrency\":%zu,\"read_buffer_size\":%zu,"
           "\"libcurl_mode\":\"multi\",\"connection_cache\":\"multi\","
           "\"libcurl_pipewait\":%ld,"
           "\"libcurl_max_host_connections\":%ld,"
           "\"libcurl_max_total_connections\":%ld,"
           "\"libcurl_max_concurrent_streams\":%ld,"
           "\"http_version\":\"%s\",\"bytes\":%llu,"
           "\"elapsed_ms\":%.3f,\"rps\":%.3f,"
           "\"latency_us_p50\":%llu,\"latency_us_p90\":%llu,"
           "\"latency_us_p95\":%llu,\"latency_us_p99\":%llu,"
           "\"latency_us_p999\":%llu}\n",
           config->url, config->proxy, config->requests, config->warmup_requests,
           duration_seconds, result.completed, result.errors, config->concurrency,
           config->read_buffer_size, config->libcurl_pipewait,
           config->libcurl_max_host_connections, config->libcurl_max_total_connections,
           config->libcurl_max_concurrent_streams, config->http_version,
           (unsigned long long)result.bytes,
           (double)elapsed_us / 1000.0, rps,
           (unsigned long long)percentile(result.latencies_us, result.completed, 50),
           (unsigned long long)percentile(result.latencies_us, result.completed, 90),
           (unsigned long long)percentile(result.latencies_us, result.completed, 95),
           (unsigned long long)percentile(result.latencies_us, result.completed, 99),
           (unsigned long long)percentile_permille(result.latencies_us, result.completed, 999));

    cleanup_multi_transfers(transfers, config->concurrency);
    curl_multi_cleanup(multi);
    free(transfers);
    free(result.latencies_us);
    return result.errors == 0 ? 0 : 1;
}

static int run_threaded_benchmark(const Config *config)
{
    pthread_t *threads = calloc(config->concurrency, sizeof(pthread_t));
    Worker *workers = calloc(config->concurrency, sizeof(Worker));
    uint64_t *latencies = NULL;
    pthread_barrier_t ready_barrier;
    pthread_barrier_t start_barrier;
    pthread_mutex_t share_lock;
    CURLSH *share = NULL;
    int share_lock_initialized = 0;
    if (threads == NULL || workers == NULL) {
        fprintf(stderr, "allocation failed\n");
        return 2;
    }
    if (pthread_barrier_init(&ready_barrier, NULL, (unsigned int)config->concurrency + 1) != 0) {
        fprintf(stderr, "barrier init failed\n");
        return 2;
    }
    if (pthread_barrier_init(&start_barrier, NULL, (unsigned int)config->concurrency + 1) != 0) {
        fprintf(stderr, "barrier init failed\n");
        pthread_barrier_destroy(&ready_barrier);
        return 2;
    }

    if (config->share_connections) {
        CURLSHcode share_result;
        if (pthread_mutex_init(&share_lock, NULL) != 0) {
            fprintf(stderr, "share mutex init failed\n");
            return 2;
        }
        share_lock_initialized = 1;
        share = curl_share_init();
        if (share == NULL) {
            fprintf(stderr, "curl_share_init failed\n");
            return 2;
        }
        share_result = curl_share_setopt(share, CURLSHOPT_SHARE, CURL_LOCK_DATA_CONNECT);
        if (share_result == CURLSHE_OK) {
            share_result = curl_share_setopt(share, CURLSHOPT_LOCKFUNC, curl_lock);
        }
        if (share_result == CURLSHE_OK) {
            share_result = curl_share_setopt(share, CURLSHOPT_UNLOCKFUNC, curl_unlock);
        }
        if (share_result == CURLSHE_OK) {
            share_result = curl_share_setopt(share, CURLSHOPT_USERDATA, &share_lock);
        }
        if (share_result != CURLSHE_OK) {
            fprintf(stderr, "curl_share_setopt failed: %s\n", curl_share_strerror(share_result));
            return 2;
        }
    }

    for (size_t i = 0; i < config->concurrency; i++) {
        size_t count = config->requests / config->concurrency;
        if (i < config->requests % config->concurrency) {
            count++;
        }
        size_t warmup_count = config->warmup_requests / config->concurrency;
        if (i < config->warmup_requests % config->concurrency) {
            warmup_count++;
        }
        workers[i].config = config;
        workers[i].ready_barrier = &ready_barrier;
        workers[i].start_barrier = &start_barrier;
        workers[i].count = count;
        workers[i].warmup_count = warmup_count;
        workers[i].latency_capacity = config->duration_seconds > 0 || count == 0 ? 1024 : count;
        workers[i].latencies_us = calloc(workers[i].latency_capacity, sizeof(uint64_t));
        workers[i].share = share;
        if (workers[i].latencies_us == NULL) {
            fprintf(stderr, "allocation failed\n");
            return 2;
        }
        int create_result = pthread_create(&threads[i], NULL, run_worker, &workers[i]);
        if (create_result != 0) {
            fprintf(stderr, "pthread_create failed for worker %zu: %s\n", i,
                    strerror(create_result));
            return 2;
        }
    }

    pthread_barrier_wait(&ready_barrier);
    uint64_t started = now_us();
    pthread_barrier_wait(&start_barrier);

    size_t errors = 0;
    uint64_t bytes = 0;
    size_t completed = 0;
    for (size_t i = 0; i < config->concurrency; i++) {
        pthread_join(threads[i], NULL);
        errors += workers[i].errors;
        bytes += workers[i].bytes;
        completed += workers[i].completed;
    }
    uint64_t elapsed_us = now_us() - started;

    if (completed > 0) {
        latencies = calloc(completed, sizeof(uint64_t));
        if (latencies == NULL) {
            fprintf(stderr, "allocation failed\n");
            return 2;
        }
        size_t cursor = 0;
        for (size_t i = 0; i < config->concurrency; i++) {
            memmove(&latencies[cursor], workers[i].latencies_us,
                    workers[i].completed * sizeof(uint64_t));
            cursor += workers[i].completed;
        }
    }
    if (completed > 0) {
        qsort(latencies, completed, sizeof(uint64_t), cmp_u64);
    }

    char duration_seconds[32];
    if (config->duration_seconds > 0) {
        snprintf(duration_seconds, sizeof(duration_seconds), "%llu",
                 (unsigned long long)config->duration_seconds);
    } else {
        snprintf(duration_seconds, sizeof(duration_seconds), "null");
    }

    double rps = elapsed_us == 0 ? 0.0 : (double)completed * 1000000.0 / (double)elapsed_us;
    printf("{\"client\":\"libcurl\",\"url\":\"%s\",\"proxy\":\"%s\","
           "\"requests\":%zu,\"warmup_requests\":%zu,\"duration_seconds\":%s,\"completed\":%zu,"
           "\"errors\":%zu,\"concurrency\":%zu,\"read_buffer_size\":%zu,"
           "\"libcurl_mode\":\"easy-threads\",\"connection_cache\":\"%s\","
           "\"libcurl_pipewait\":%ld,"
           "\"http_version\":\"%s\",\"bytes\":%llu,"
           "\"elapsed_ms\":%.3f,\"rps\":%.3f,"
           "\"latency_us_p50\":%llu,\"latency_us_p90\":%llu,"
           "\"latency_us_p95\":%llu,\"latency_us_p99\":%llu,"
           "\"latency_us_p999\":%llu}\n",
           config->url, config->proxy, config->requests, config->warmup_requests, duration_seconds,
           completed, errors, config->concurrency, config->read_buffer_size,
           config->share_connections ? "shared" : "per-thread", config->libcurl_pipewait,
           config->http_version, (unsigned long long)bytes,
           (double)elapsed_us / 1000.0, rps,
           (unsigned long long)percentile(latencies, completed, 50),
           (unsigned long long)percentile(latencies, completed, 90),
           (unsigned long long)percentile(latencies, completed, 95),
           (unsigned long long)percentile(latencies, completed, 99),
           (unsigned long long)percentile_permille(latencies, completed, 999));

    if (share != NULL) {
        curl_share_cleanup(share);
    }
    if (share_lock_initialized) {
        pthread_mutex_destroy(&share_lock);
    }
    pthread_barrier_destroy(&ready_barrier);
    pthread_barrier_destroy(&start_barrier);
    free(latencies);
    for (size_t i = 0; i < config->concurrency; i++) {
        free(workers[i].latencies_us);
    }
    free(workers);
    free(threads);
    return errors == 0 ? 0 : 1;
}

int main(int argc, char **argv)
{
    Config config = parse_args(argc, argv);
    curl_global_init(CURL_GLOBAL_DEFAULT);

    int result;
    if (strcmp(config.libcurl_mode, "multi") == 0) {
        result = run_multi_benchmark(&config);
    } else {
        result = run_threaded_benchmark(&config);
    }

    curl_global_cleanup();
    return result;
}
