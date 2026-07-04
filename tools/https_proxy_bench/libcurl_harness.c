/*
 * Copyright (c) 2026 Huawei Device Co., Ltd.
 * Licensed under the Apache License, Version 2.0.
 */

#include <curl/curl.h>
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
    long proxy_type;
    long insecure_proxy;
    long insecure_origin;
    int share_connections;
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

static size_t write_body(char *ptr, size_t size, size_t nmemb, void *userdata)
{
    Worker *worker = (Worker *)userdata;
    size_t total = size * nmemb;
    (void)ptr;
    worker->bytes += (uint64_t)total;
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

static void set_common_options(CURL *curl, Worker *worker)
{
    const Config *config = worker->config;

    curl_easy_setopt(curl, CURLOPT_URL, config->url);
    curl_easy_setopt(curl, CURLOPT_PROXY, config->proxy);
    curl_easy_setopt(curl, CURLOPT_PROXYTYPE, config->proxy_type);
    curl_easy_setopt(curl, CURLOPT_HTTP_VERSION, CURL_HTTP_VERSION_1_1);
    curl_easy_setopt(curl, CURLOPT_NOSIGNAL, 1L);
    curl_easy_setopt(curl, CURLOPT_WRITEFUNCTION, write_body);
    curl_easy_setopt(curl, CURLOPT_WRITEDATA, worker);
    curl_easy_setopt(curl, CURLOPT_BUFFERSIZE, (long)config->read_buffer_size);
    if (worker->share != NULL) {
        curl_easy_setopt(curl, CURLOPT_SHARE, worker->share);
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

static void usage(const char *program)
{
    fprintf(stderr,
            "usage: %s --url URL --proxy http[s]://PROXY[:PORT] [--requests N] "
            "[--warmup-requests N] [--duration-seconds N] [--concurrency N] "
            "[--read-buffer-size N] "
            "[--proxy-ca-file PEM] [--proxy-client-cert PEM] [--proxy-client-key PEM] "
            "[--origin-ca-file PEM] [--insecure-proxy] [--insecure-origin] "
            "[--proxy-user-pass user:pass] [--share-connections]\n",
            program);
}

static Config parse_args(int argc, char **argv)
{
    Config config;
    memset(&config, 0, sizeof(config));
    config.requests = 1000;
    config.concurrency = 16;
    config.read_buffer_size = 64 * 1024;

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
        } else if (strcmp(argv[i], "--share-connections") == 0) {
            config.share_connections = 1;
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

int main(int argc, char **argv)
{
    Config config = parse_args(argc, argv);
    pthread_t *threads = calloc(config.concurrency, sizeof(pthread_t));
    Worker *workers = calloc(config.concurrency, sizeof(Worker));
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
    if (pthread_barrier_init(&ready_barrier, NULL, (unsigned int)config.concurrency + 1) != 0) {
        fprintf(stderr, "barrier init failed\n");
        return 2;
    }
    if (pthread_barrier_init(&start_barrier, NULL, (unsigned int)config.concurrency + 1) != 0) {
        fprintf(stderr, "barrier init failed\n");
        pthread_barrier_destroy(&ready_barrier);
        return 2;
    }

    curl_global_init(CURL_GLOBAL_DEFAULT);
    if (config.share_connections) {
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

    for (size_t i = 0; i < config.concurrency; i++) {
        size_t count = config.requests / config.concurrency;
        if (i < config.requests % config.concurrency) {
            count++;
        }
        size_t warmup_count = config.warmup_requests / config.concurrency;
        if (i < config.warmup_requests % config.concurrency) {
            warmup_count++;
        }
        workers[i].config = &config;
        workers[i].ready_barrier = &ready_barrier;
        workers[i].start_barrier = &start_barrier;
        workers[i].count = count;
        workers[i].warmup_count = warmup_count;
        workers[i].latency_capacity = config.duration_seconds > 0 || count == 0 ? 1024 : count;
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
    for (size_t i = 0; i < config.concurrency; i++) {
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
        for (size_t i = 0; i < config.concurrency; i++) {
            memmove(&latencies[cursor], workers[i].latencies_us,
                    workers[i].completed * sizeof(uint64_t));
            cursor += workers[i].completed;
        }
    }
    if (completed > 0) {
        qsort(latencies, completed, sizeof(uint64_t), cmp_u64);
    }

    char duration_seconds[32];
    if (config.duration_seconds > 0) {
        snprintf(duration_seconds, sizeof(duration_seconds), "%llu",
                 (unsigned long long)config.duration_seconds);
    } else {
        snprintf(duration_seconds, sizeof(duration_seconds), "null");
    }

    double rps = elapsed_us == 0 ? 0.0 : (double)completed * 1000000.0 / (double)elapsed_us;
    printf("{\"client\":\"libcurl\",\"url\":\"%s\",\"proxy\":\"%s\","
           "\"requests\":%zu,\"warmup_requests\":%zu,\"duration_seconds\":%s,\"completed\":%zu,"
           "\"errors\":%zu,\"concurrency\":%zu,\"read_buffer_size\":%zu,"
           "\"connection_cache\":\"%s\",\"bytes\":%llu,\"elapsed_ms\":%.3f,\"rps\":%.3f,"
           "\"latency_us_p50\":%llu,\"latency_us_p90\":%llu,"
           "\"latency_us_p95\":%llu,\"latency_us_p99\":%llu,"
           "\"latency_us_p999\":%llu}\n",
           config.url, config.proxy, config.requests, config.warmup_requests, duration_seconds,
           completed, errors, config.concurrency, config.read_buffer_size,
           config.share_connections ? "shared" : "per-thread", (unsigned long long)bytes,
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
    curl_global_cleanup();
    pthread_barrier_destroy(&ready_barrier);
    pthread_barrier_destroy(&start_barrier);
    free(latencies);
    for (size_t i = 0; i < config.concurrency; i++) {
        free(workers[i].latencies_us);
    }
    free(workers);
    free(threads);
    return errors == 0 ? 0 : 1;
}
