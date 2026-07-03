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
    size_t requests;
    size_t warmup_requests;
    size_t concurrency;
    size_t read_buffer_size;
} Config;

typedef struct {
    const Config *config;
    pthread_barrier_t *ready_barrier;
    pthread_barrier_t *start_barrier;
    size_t start;
    size_t count;
    size_t warmup_count;
    uint64_t *latencies_us;
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
        worker->errors += worker->warmup_count + worker->count;
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

    pthread_barrier_wait(worker->ready_barrier);
    pthread_barrier_wait(worker->start_barrier);

    for (size_t i = 0; i < worker->count; i++) {
        uint64_t started = now_us();
        if (curl_easy_perform(curl) == CURLE_OK) {
            worker->latencies_us[worker->start + worker->completed] = now_us() - started;
            worker->completed++;
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
            "[--warmup-requests N] [--concurrency N] [--read-buffer-size N] "
            "[--proxy-ca-file PEM] [--proxy-client-cert PEM] [--proxy-client-key PEM] "
            "[--origin-ca-file PEM] [--insecure-proxy] [--insecure-origin] "
            "[--proxy-user-pass user:pass]\n",
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

    if (config.url == NULL || config.proxy == NULL || config.requests == 0 ||
        config.concurrency == 0 || config.read_buffer_size == 0) {
        usage(argv[0]);
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
    uint64_t *latencies = calloc(config.requests, sizeof(uint64_t));
    pthread_barrier_t ready_barrier;
    pthread_barrier_t start_barrier;
    if (threads == NULL || workers == NULL || latencies == NULL) {
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

    size_t cursor = 0;
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
        workers[i].start = cursor;
        workers[i].count = count;
        workers[i].warmup_count = warmup_count;
        workers[i].latencies_us = latencies;
        cursor += count;
        pthread_create(&threads[i], NULL, run_worker, &workers[i]);
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
        memmove(&latencies[completed], &latencies[workers[i].start],
                workers[i].completed * sizeof(uint64_t));
        completed += workers[i].completed;
    }
    uint64_t elapsed_us = now_us() - started;

    if (completed > 0) {
        qsort(latencies, completed, sizeof(uint64_t), cmp_u64);
    }

    double rps = elapsed_us == 0 ? 0.0 : (double)completed * 1000000.0 / (double)elapsed_us;
    printf("{\"client\":\"libcurl\",\"url\":\"%s\",\"proxy\":\"%s\","
           "\"requests\":%zu,\"warmup_requests\":%zu,\"completed\":%zu,"
           "\"errors\":%zu,\"concurrency\":%zu,\"read_buffer_size\":%zu,"
           "\"bytes\":%llu,\"elapsed_ms\":%.3f,\"rps\":%.3f,"
           "\"latency_us_p50\":%llu,\"latency_us_p90\":%llu,"
           "\"latency_us_p95\":%llu,\"latency_us_p99\":%llu}\n",
           config.url, config.proxy, config.requests, config.warmup_requests, completed, errors,
           config.concurrency, config.read_buffer_size, (unsigned long long)bytes,
           (double)elapsed_us / 1000.0, rps,
           (unsigned long long)percentile(latencies, completed, 50),
           (unsigned long long)percentile(latencies, completed, 90),
           (unsigned long long)percentile(latencies, completed, 95),
           (unsigned long long)percentile(latencies, completed, 99));

    curl_global_cleanup();
    pthread_barrier_destroy(&ready_barrier);
    pthread_barrier_destroy(&start_barrier);
    free(latencies);
    free(workers);
    free(threads);
    return errors == 0 ? 0 : 1;
}
