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
    const char *method;
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
    size_t runtime_threads;
    size_t read_buffer_size;
    size_t body_size;
} Config;

typedef struct {
    const Config *config;
    pthread_barrier_t *ready_barrier;
    pthread_barrier_t *start_barrier;
    uint64_t *started_us;
    size_t start;
    size_t count;
    size_t warmup_count;
    uint64_t *latencies_us;
    uint64_t *connect_us;
    uint64_t *appconnect_us;
    uint64_t *starttransfer_us;
    uint64_t *total_time_us;
    uint64_t *body_transfer_us;
    char *body;
    struct curl_slist *headers;
    uint64_t bytes;
    uint64_t body_reads;
    uint64_t start_delay_us;
    uint64_t elapsed_us;
    size_t completed;
    size_t errors;
} Worker;

static size_t write_body(char *ptr, size_t size, size_t nmemb, void *userdata)
{
    Worker *worker = (Worker *)userdata;
    size_t total = size * nmemb;
    (void)ptr;
    worker->bytes += (uint64_t)total;
    worker->body_reads++;
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

static uint64_t curl_time_us(CURL *curl, CURLINFO info)
{
    curl_off_t us = 0;
    CURLcode code = curl_easy_getinfo(curl, info, &us);
    if (code != CURLE_OK || us <= 0) {
        return 0;
    }
    return (uint64_t)us;
}

static void set_common_options(CURL *curl, Worker *worker)
{
    const Config *config = worker->config;

    curl_easy_setopt(curl, CURLOPT_URL, config->url);
    if (config->proxy != NULL) {
        curl_easy_setopt(curl, CURLOPT_PROXY, config->proxy);
        curl_easy_setopt(curl, CURLOPT_PROXYTYPE, config->proxy_type);
    } else {
        curl_easy_setopt(curl, CURLOPT_PROXY, "");
    }
    curl_easy_setopt(curl, CURLOPT_HTTP_VERSION, CURL_HTTP_VERSION_1_1);
    curl_easy_setopt(curl, CURLOPT_NOSIGNAL, 1L);
    curl_easy_setopt(curl, CURLOPT_WRITEFUNCTION, write_body);
    curl_easy_setopt(curl, CURLOPT_WRITEDATA, worker);
    curl_easy_setopt(curl, CURLOPT_BUFFERSIZE, (long)config->read_buffer_size);

    if (strcmp(config->method, "POST") == 0) {
        worker->headers = curl_slist_append(worker->headers, "Expect:");
        curl_easy_setopt(curl, CURLOPT_HTTPHEADER, worker->headers);
        curl_easy_setopt(curl, CURLOPT_POST, 1L);
        curl_easy_setopt(curl, CURLOPT_POSTFIELDSIZE_LARGE, (curl_off_t)config->body_size);
        curl_easy_setopt(curl, CURLOPT_POSTFIELDS, worker->body);
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

static uint64_t wait_for_measured_start(Worker *worker)
{
    pthread_barrier_wait(worker->ready_barrier);
    pthread_barrier_wait(worker->start_barrier);
    uint64_t measured_started = *worker->started_us;
    uint64_t started_now = now_us();
    worker->start_delay_us =
        started_now >= measured_started ? started_now - measured_started : 0;
    return measured_started;
}

static void *run_worker(void *arg)
{
    Worker *worker = (Worker *)arg;
    CURL *curl = curl_easy_init();
    if (curl == NULL) {
        worker->errors += worker->warmup_count + worker->count;
        wait_for_measured_start(worker);
        return NULL;
    }

    if (strcmp(worker->config->method, "POST") == 0 && worker->config->body_size > 0) {
        worker->body = malloc(worker->config->body_size);
        if (worker->body == NULL) {
            worker->errors += worker->warmup_count + worker->count;
            wait_for_measured_start(worker);
            curl_easy_cleanup(curl);
            return NULL;
        }
        memset(worker->body, 'x', worker->config->body_size);
    }

    set_common_options(curl, worker);

    for (size_t i = 0; i < worker->warmup_count; i++) {
        CURLcode code = curl_easy_perform(curl);
        if (code != CURLE_OK) {
            worker->errors++;
        }
    }
    worker->bytes = 0;
    worker->body_reads = 0;
    uint64_t measured_started = wait_for_measured_start(worker);

    for (size_t i = 0; i < worker->count; i++) {
        uint64_t started = now_us();
        CURLcode code = curl_easy_perform(curl);
        if (code == CURLE_OK) {
            size_t index = worker->start + worker->completed;
            uint64_t total_us = curl_time_us(curl, CURLINFO_TOTAL_TIME_T);
            uint64_t starttransfer_us = curl_time_us(curl, CURLINFO_STARTTRANSFER_TIME_T);
            worker->latencies_us[index] = now_us() - started;
            worker->connect_us[index] = curl_time_us(curl, CURLINFO_CONNECT_TIME_T);
            worker->appconnect_us[index] = curl_time_us(curl, CURLINFO_APPCONNECT_TIME_T);
            worker->starttransfer_us[index] = starttransfer_us;
            worker->total_time_us[index] = total_us;
            worker->body_transfer_us[index] =
                total_us > starttransfer_us ? total_us - starttransfer_us : 0;
            worker->completed++;
        } else {
            worker->errors++;
        }
    }
    worker->elapsed_us = now_us() - measured_started;

    curl_easy_cleanup(curl);
    curl_slist_free_all(worker->headers);
    free(worker->body);
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
            "usage: %s --url URL [--proxy http[s]://PROXY[:PORT]] [--requests N] "
            "[--concurrency N] [--proxy-ca-file PEM] [--proxy-client-cert PEM] "
            "[--proxy-client-key PEM] [--origin-ca-file PEM] [--insecure-proxy] "
            "[--insecure-origin] [--proxy-user-pass user:pass] [--method GET|POST] "
            "[--body-size N] [--warmup-requests N] [--runtime-threads N] "
            "[--read-buffer-size N]\n",
            program);
}

static Config parse_args(int argc, char **argv)
{
    Config config;
    memset(&config, 0, sizeof(config));
    config.requests = 1000;
    config.concurrency = 16;
    config.runtime_threads = 0;
    config.read_buffer_size = 64 * 1024;
    config.method = "GET";

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
        } else if (strcmp(argv[i], "--runtime-threads") == 0) {
            config.runtime_threads =
                strtoull(next_value(&i, argc, argv, "--runtime-threads"), NULL, 10);
        } else if (strcmp(argv[i], "--read-buffer-size") == 0) {
            config.read_buffer_size =
                strtoull(next_value(&i, argc, argv, "--read-buffer-size"), NULL, 10);
        } else if (strcmp(argv[i], "--method") == 0) {
            config.method = next_value(&i, argc, argv, "--method");
        } else if (strcmp(argv[i], "--body-size") == 0) {
            config.body_size = strtoull(next_value(&i, argc, argv, "--body-size"), NULL, 10);
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

    if (config.url == NULL || config.requests == 0 || config.concurrency == 0 ||
        config.read_buffer_size == 0) {
        usage(argv[0]);
        exit(2);
    }
    if (config.proxy != NULL) {
        if (strncmp(config.proxy, "https://", strlen("https://")) == 0) {
            config.proxy_type = CURLPROXY_HTTPS;
        } else if (strncmp(config.proxy, "http://", strlen("http://")) == 0) {
            config.proxy_type = CURLPROXY_HTTP;
        } else {
            fprintf(stderr, "--proxy must start with http:// or https://\n");
            exit(2);
        }
    }
    if (config.runtime_threads == 0) {
        config.runtime_threads = config.concurrency;
    }
    if (strcmp(config.method, "GET") != 0 && strcmp(config.method, "POST") != 0) {
        fprintf(stderr, "--method must be GET or POST\n");
        exit(2);
    }
    if (strcmp(config.method, "GET") == 0 && config.body_size != 0) {
        fprintf(stderr, "--body-size is only supported with --method POST\n");
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
    uint64_t *connect_us = calloc(config.requests, sizeof(uint64_t));
    uint64_t *appconnect_us = calloc(config.requests, sizeof(uint64_t));
    uint64_t *starttransfer_us = calloc(config.requests, sizeof(uint64_t));
    uint64_t *total_time_us = calloc(config.requests, sizeof(uint64_t));
    uint64_t *body_transfer_us = calloc(config.requests, sizeof(uint64_t));
    pthread_barrier_t ready_barrier;
    pthread_barrier_t start_barrier;
    uint64_t started = 0;
    if (threads == NULL || workers == NULL || latencies == NULL || connect_us == NULL ||
        appconnect_us == NULL || starttransfer_us == NULL || total_time_us == NULL ||
        body_transfer_us == NULL) {
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
        workers[i].config = &config;
        workers[i].ready_barrier = &ready_barrier;
        workers[i].start_barrier = &start_barrier;
        workers[i].started_us = &started;
        workers[i].start = cursor;
        workers[i].count = count;
        workers[i].warmup_count = config.warmup_requests / config.concurrency;
        if (i < config.warmup_requests % config.concurrency) {
            workers[i].warmup_count++;
        }
        workers[i].latencies_us = latencies;
        workers[i].connect_us = connect_us;
        workers[i].appconnect_us = appconnect_us;
        workers[i].starttransfer_us = starttransfer_us;
        workers[i].total_time_us = total_time_us;
        workers[i].body_transfer_us = body_transfer_us;
        cursor += count;
        pthread_create(&threads[i], NULL, run_worker, &workers[i]);
    }
    pthread_barrier_wait(&ready_barrier);
    started = now_us();
    pthread_barrier_wait(&start_barrier);

    size_t errors = 0;
    uint64_t bytes = 0;
    uint64_t body_reads = 0;
    uint64_t worker_start_delay_us_min = 0;
    uint64_t worker_start_delay_us_max = 0;
    uint64_t worker_elapsed_us_min = 0;
    uint64_t worker_elapsed_us_max = 0;
    for (size_t i = 0; i < config.concurrency; i++) {
        pthread_join(threads[i], NULL);
        errors += workers[i].errors;
        bytes += workers[i].bytes;
        body_reads += workers[i].body_reads;
        if (worker_start_delay_us_min == 0 ||
            workers[i].start_delay_us < worker_start_delay_us_min) {
            worker_start_delay_us_min = workers[i].start_delay_us;
        }
        if (workers[i].start_delay_us > worker_start_delay_us_max) {
            worker_start_delay_us_max = workers[i].start_delay_us;
        }
        if (workers[i].elapsed_us != 0 &&
            (worker_elapsed_us_min == 0 || workers[i].elapsed_us < worker_elapsed_us_min)) {
            worker_elapsed_us_min = workers[i].elapsed_us;
        }
        if (workers[i].elapsed_us > worker_elapsed_us_max) {
            worker_elapsed_us_max = workers[i].elapsed_us;
        }
    }
    uint64_t elapsed_us = now_us() - started;
    size_t compact = 0;
    for (size_t i = 0; i < config.concurrency; i++) {
        memmove(&latencies[compact], &latencies[workers[i].start],
                workers[i].completed * sizeof(uint64_t));
        memmove(&connect_us[compact], &connect_us[workers[i].start],
                workers[i].completed * sizeof(uint64_t));
        memmove(&appconnect_us[compact], &appconnect_us[workers[i].start],
                workers[i].completed * sizeof(uint64_t));
        memmove(&starttransfer_us[compact], &starttransfer_us[workers[i].start],
                workers[i].completed * sizeof(uint64_t));
        memmove(&total_time_us[compact], &total_time_us[workers[i].start],
                workers[i].completed * sizeof(uint64_t));
        memmove(&body_transfer_us[compact], &body_transfer_us[workers[i].start],
                workers[i].completed * sizeof(uint64_t));
        compact += workers[i].completed;
    }
    size_t completed = compact;

    if (completed > 0) {
        qsort(latencies, completed, sizeof(uint64_t), cmp_u64);
        qsort(connect_us, completed, sizeof(uint64_t), cmp_u64);
        qsort(appconnect_us, completed, sizeof(uint64_t), cmp_u64);
        qsort(starttransfer_us, completed, sizeof(uint64_t), cmp_u64);
        qsort(total_time_us, completed, sizeof(uint64_t), cmp_u64);
        qsort(body_transfer_us, completed, sizeof(uint64_t), cmp_u64);
    }

    double elapsed_ms = (double)elapsed_us / 1000.0;
    double rps = elapsed_us == 0 ? 0.0 : (double)completed * 1000000.0 / (double)elapsed_us;

    printf("{\"client\":\"libcurl\",\"url\":\"%s\",\"proxy\":\"%s\",\"method\":\"%s\","
           "\"body_size\":%zu,\"requests\":%zu,\"warmup_requests\":%zu,"
           "\"completed\":%zu,\"errors\":%zu,"
           "\"concurrency\":%zu,\"runtime_threads\":%zu,\"read_buffer_size\":%zu,\"bytes\":%llu,"
           "\"body_reads\":%llu,\"avg_body_read_size\":%.3f,"
           "\"elapsed_ms\":%.3f,\"rps\":%.3f,\"latency_us_p50\":%llu,"
           "\"latency_us_p90\":%llu,\"latency_us_p95\":%llu,\"latency_us_p99\":%llu,"
           "\"connect_us_p50\":%llu,\"connect_us_p90\":%llu,\"connect_us_p99\":%llu,"
           "\"appconnect_us_p50\":%llu,\"appconnect_us_p90\":%llu,\"appconnect_us_p99\":%llu,"
           "\"starttransfer_us_p50\":%llu,\"starttransfer_us_p90\":%llu,"
           "\"starttransfer_us_p99\":%llu,\"total_time_us_p50\":%llu,"
           "\"total_time_us_p90\":%llu,\"total_time_us_p99\":%llu,"
           "\"body_transfer_us_p50\":%llu,\"body_transfer_us_p90\":%llu,"
           "\"body_transfer_us_p99\":%llu,"
           "\"worker_start_delay_us_min\":%llu,\"worker_start_delay_us_max\":%llu,"
           "\"worker_elapsed_us_min\":%llu,\"worker_elapsed_us_max\":%llu}\n",
           config.url, config.proxy == NULL ? "" : config.proxy, config.method, config.body_size, config.requests,
           config.warmup_requests, completed, errors, config.concurrency,
           config.runtime_threads, config.read_buffer_size, (unsigned long long)bytes,
           (unsigned long long)body_reads,
           body_reads == 0 ? 0.0 : (double)bytes / (double)body_reads, elapsed_ms, rps,
           (unsigned long long)percentile(latencies, completed, 50),
           (unsigned long long)percentile(latencies, completed, 90),
           (unsigned long long)percentile(latencies, completed, 95),
           (unsigned long long)percentile(latencies, completed, 99),
           (unsigned long long)percentile(connect_us, completed, 50),
           (unsigned long long)percentile(connect_us, completed, 90),
           (unsigned long long)percentile(connect_us, completed, 99),
           (unsigned long long)percentile(appconnect_us, completed, 50),
           (unsigned long long)percentile(appconnect_us, completed, 90),
           (unsigned long long)percentile(appconnect_us, completed, 99),
           (unsigned long long)percentile(starttransfer_us, completed, 50),
           (unsigned long long)percentile(starttransfer_us, completed, 90),
           (unsigned long long)percentile(starttransfer_us, completed, 99),
           (unsigned long long)percentile(total_time_us, completed, 50),
           (unsigned long long)percentile(total_time_us, completed, 90),
           (unsigned long long)percentile(total_time_us, completed, 99),
           (unsigned long long)percentile(body_transfer_us, completed, 50),
           (unsigned long long)percentile(body_transfer_us, completed, 90),
           (unsigned long long)percentile(body_transfer_us, completed, 99),
           (unsigned long long)worker_start_delay_us_min,
           (unsigned long long)worker_start_delay_us_max,
           (unsigned long long)worker_elapsed_us_min,
           (unsigned long long)worker_elapsed_us_max);

    curl_global_cleanup();
    pthread_barrier_destroy(&ready_barrier);
    pthread_barrier_destroy(&start_barrier);
    free(body_transfer_us);
    free(total_time_us);
    free(starttransfer_us);
    free(appconnect_us);
    free(connect_us);
    free(latencies);
    free(workers);
    free(threads);
    return errors == 0 ? 0 : 1;
}
