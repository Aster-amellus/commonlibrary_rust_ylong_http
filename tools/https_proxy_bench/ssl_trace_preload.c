// Copyright (c) 2026 Huawei Device Co., Ltd.
// Licensed under the Apache License, Version 2.0.
//
// LD_PRELOAD helper for HTTPS proxy benchmark diagnostics. It records OpenSSL
// read/write size histograms without requiring tracefs or bpftrace privileges.

#define _GNU_SOURCE

#include <dlfcn.h>
#include <inttypes.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

typedef struct ssl_st SSL;
typedef struct bio_st BIO;

typedef int (*ssl_read_fn)(SSL *, void *, int);
typedef int (*ssl_read_ex_fn)(SSL *, void *, size_t, size_t *);
typedef int (*ssl_write_fn)(SSL *, const void *, int);
typedef int (*ssl_write_ex_fn)(SSL *, const void *, size_t, size_t *);
typedef int (*ssl_connect_fn)(SSL *);
typedef int (*ssl_shutdown_fn)(SSL *);
typedef int (*ssl_get_error_fn)(const SSL *, int);
typedef int (*bio_read_fn)(BIO *, void *, int);
typedef int (*bio_write_fn)(BIO *, const void *, int);

#define HIST_BINS 65
#define SSL_ERROR_BINS 16
#define SSL_TRACK_SLOTS 8192

enum ssl_op_kind {
    SSL_OP_NONE = 0,
    SSL_OP_READ,
    SSL_OP_READ_EX,
    SSL_OP_WRITE,
    SSL_OP_WRITE_EX,
    SSL_OP_CONNECT,
    SSL_OP_SHUTDOWN,
    SSL_OP_OTHER,
};

enum ssl_retry_gap_kind {
    SSL_RETRY_GAP_READ_DEPTH1 = 0,
    SSL_RETRY_GAP_READ_DEPTH2,
    SSL_RETRY_GAP_READ_DEPTH_OTHER,
    SSL_RETRY_GAP_CONNECT,
    SSL_RETRY_GAP_OTHER,
    SSL_RETRY_GAP_KIND_COUNT,
};

struct ssl_retry_slot {
    const SSL *ssl;
    unsigned long long want_read_ns;
    unsigned long long want_write_ns;
    pthread_t want_read_thread;
    pthread_t want_write_thread;
};

struct ssl_retry_gap_sample {
    const SSL *ssl;
    unsigned long long gap_us;
    unsigned long long want_thread;
    unsigned long long retry_thread;
    int same_thread;
};

static ssl_read_fn real_ssl_read;
static ssl_read_ex_fn real_ssl_read_ex;
static ssl_write_fn real_ssl_write;
static ssl_write_ex_fn real_ssl_write_ex;
static ssl_connect_fn real_ssl_connect;
static ssl_shutdown_fn real_ssl_shutdown;
static ssl_get_error_fn real_ssl_get_error;
static bio_read_fn real_bio_read;
static bio_write_fn real_bio_write;
static pthread_once_t resolve_once = PTHREAD_ONCE_INIT;

static __thread enum ssl_op_kind tls_last_op;
static __thread const SSL *tls_last_ssl;
static __thread int tls_last_ret;
static __thread int tls_ssl_read_depth;

static unsigned long long ssl_read_calls;
static unsigned long long ssl_read_ex_calls;
static unsigned long long ssl_write_calls;
static unsigned long long ssl_write_ex_calls;
static unsigned long long ssl_connect_calls;
static unsigned long long ssl_shutdown_calls;
static unsigned long long ssl_get_error_calls;
static unsigned long long ssl_read_errors;
static unsigned long long ssl_write_errors;
static unsigned long long ssl_connect_errors;
static unsigned long long ssl_shutdown_errors;
static unsigned long long ssl_read_requested_bytes;
static unsigned long long ssl_read_returned_bytes;
static unsigned long long ssl_write_requested_bytes;
static unsigned long long ssl_write_returned_bytes;
static unsigned long long bio_read_calls;
static unsigned long long bio_write_calls;
static unsigned long long bio_read_errors;
static unsigned long long bio_write_errors;
static unsigned long long bio_read_requested_bytes;
static unsigned long long bio_read_returned_bytes;
static unsigned long long bio_write_requested_bytes;
static unsigned long long bio_write_returned_bytes;
static unsigned long long ssl_read_requested_hist[HIST_BINS];
static unsigned long long ssl_read_returned_hist[HIST_BINS];
static unsigned long long ssl_write_requested_hist[HIST_BINS];
static unsigned long long ssl_write_returned_hist[HIST_BINS];
static unsigned long long bio_read_requested_hist[HIST_BINS];
static unsigned long long bio_read_returned_hist[HIST_BINS];
static unsigned long long bio_write_requested_hist[HIST_BINS];
static unsigned long long bio_write_returned_hist[HIST_BINS];
static unsigned long long ssl_error_code_hist[SSL_ERROR_BINS];
static unsigned long long ssl_read_error_code_hist[SSL_ERROR_BINS];
static unsigned long long ssl_write_error_code_hist[SSL_ERROR_BINS];
static unsigned long long ssl_connect_error_code_hist[SSL_ERROR_BINS];
static unsigned long long ssl_shutdown_error_code_hist[SSL_ERROR_BINS];
static unsigned long long ssl_other_error_code_hist[SSL_ERROR_BINS];
static struct ssl_retry_slot ssl_retry_slots[SSL_TRACK_SLOTS];
static pthread_mutex_t ssl_retry_lock = PTHREAD_MUTEX_INITIALIZER;
static unsigned long long ssl_retry_gap_samples[SSL_RETRY_GAP_KIND_COUNT];
static unsigned long long ssl_retry_gap_max_us[SSL_RETRY_GAP_KIND_COUNT];
static struct ssl_retry_gap_sample ssl_retry_gap_max_sample[SSL_RETRY_GAP_KIND_COUNT];
static unsigned long long ssl_retry_gap_same_thread_samples[SSL_RETRY_GAP_KIND_COUNT];
static unsigned long long ssl_retry_gap_same_thread_max_us[SSL_RETRY_GAP_KIND_COUNT];
static unsigned long long ssl_retry_gap_migrated_samples[SSL_RETRY_GAP_KIND_COUNT];
static unsigned long long ssl_retry_gap_migrated_max_us[SSL_RETRY_GAP_KIND_COUNT];
static unsigned long long ssl_retry_gap_want_write_samples;
static unsigned long long ssl_retry_gap_want_write_max_us;
static struct ssl_retry_gap_sample ssl_retry_gap_want_write_max_sample;
static unsigned long long ssl_retry_gap_want_write_same_thread_samples;
static unsigned long long ssl_retry_gap_want_write_same_thread_max_us;
static unsigned long long ssl_retry_gap_want_write_migrated_samples;
static unsigned long long ssl_retry_gap_want_write_migrated_max_us;
static unsigned long long ssl_retry_gap_read_depth1_hist[HIST_BINS];
static unsigned long long ssl_retry_gap_read_depth2_hist[HIST_BINS];
static unsigned long long ssl_retry_gap_read_depth_other_hist[HIST_BINS];
static unsigned long long ssl_retry_gap_connect_hist[HIST_BINS];
static unsigned long long ssl_retry_gap_other_hist[HIST_BINS];

static void resolve_symbols(void)
{
    real_ssl_read = (ssl_read_fn)dlsym(RTLD_NEXT, "SSL_read");
    real_ssl_read_ex = (ssl_read_ex_fn)dlsym(RTLD_NEXT, "SSL_read_ex");
    real_ssl_write = (ssl_write_fn)dlsym(RTLD_NEXT, "SSL_write");
    real_ssl_write_ex = (ssl_write_ex_fn)dlsym(RTLD_NEXT, "SSL_write_ex");
    real_ssl_connect = (ssl_connect_fn)dlsym(RTLD_NEXT, "SSL_connect");
    real_ssl_shutdown = (ssl_shutdown_fn)dlsym(RTLD_NEXT, "SSL_shutdown");
    real_ssl_get_error = (ssl_get_error_fn)dlsym(RTLD_NEXT, "SSL_get_error");
    real_bio_read = (bio_read_fn)dlsym(RTLD_NEXT, "BIO_read");
    real_bio_write = (bio_write_fn)dlsym(RTLD_NEXT, "BIO_write");
}

static unsigned int hist_bin(size_t value)
{
    unsigned int bin = 0;
    while (value != 0 && bin < HIST_BINS - 1) {
        value >>= 1;
        bin++;
    }
    return bin;
}

static void add_hist(unsigned long long hist[HIST_BINS], size_t value)
{
    __atomic_fetch_add(&hist[hist_bin(value)], 1, __ATOMIC_RELAXED);
}

static void add_counter(unsigned long long *counter, unsigned long long value)
{
    __atomic_fetch_add(counter, value, __ATOMIC_RELAXED);
}

static void update_max(unsigned long long *current, unsigned long long value)
{
    unsigned long long old = __atomic_load_n(current, __ATOMIC_RELAXED);
    while (value > old &&
           !__atomic_compare_exchange_n(
               current, &old, value, 0, __ATOMIC_RELAXED, __ATOMIC_RELAXED)) {
    }
}

static unsigned long long now_ns(void)
{
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) {
        return 0;
    }
    return (unsigned long long)ts.tv_sec * 1000000000ull + (unsigned long long)ts.tv_nsec;
}

static unsigned long long thread_id_value(pthread_t thread)
{
    unsigned long long value = 0;
    size_t copy_len = sizeof(thread) < sizeof(value) ? sizeof(thread) : sizeof(value);
    memcpy(&value, &thread, copy_len);
    return value;
}

static unsigned int ssl_error_bin(int code)
{
    if (code < 0 || code >= SSL_ERROR_BINS) {
        return SSL_ERROR_BINS - 1;
    }
    return (unsigned int)code;
}

static void remember_ssl_op(enum ssl_op_kind op, const SSL *ssl, int ret)
{
    tls_last_op = op;
    tls_last_ssl = ssl;
    tls_last_ret = ret;
}

static enum ssl_retry_gap_kind retry_gap_kind(enum ssl_op_kind op, int depth)
{
    switch (op) {
        case SSL_OP_READ:
        case SSL_OP_READ_EX:
            if (depth == 1) {
                return SSL_RETRY_GAP_READ_DEPTH1;
            }
            if (depth == 2) {
                return SSL_RETRY_GAP_READ_DEPTH2;
            }
            return SSL_RETRY_GAP_READ_DEPTH_OTHER;
        case SSL_OP_CONNECT:
            return SSL_RETRY_GAP_CONNECT;
        default:
            return SSL_RETRY_GAP_OTHER;
    }
}

static unsigned long long *retry_gap_hist(enum ssl_retry_gap_kind kind)
{
    switch (kind) {
        case SSL_RETRY_GAP_READ_DEPTH1:
            return ssl_retry_gap_read_depth1_hist;
        case SSL_RETRY_GAP_READ_DEPTH2:
            return ssl_retry_gap_read_depth2_hist;
        case SSL_RETRY_GAP_READ_DEPTH_OTHER:
            return ssl_retry_gap_read_depth_other_hist;
        case SSL_RETRY_GAP_CONNECT:
            return ssl_retry_gap_connect_hist;
        case SSL_RETRY_GAP_OTHER:
        default:
            return ssl_retry_gap_other_hist;
    }
}

static void update_retry_gap_sample(
    struct ssl_retry_gap_sample *sample,
    const SSL *ssl,
    unsigned long long gap_us,
    pthread_t want_thread,
    pthread_t retry_thread,
    int same_thread)
{
    if (gap_us > sample->gap_us) {
        sample->ssl = ssl;
        sample->gap_us = gap_us;
        sample->want_thread = thread_id_value(want_thread);
        sample->retry_thread = thread_id_value(retry_thread);
        sample->same_thread = same_thread;
    }
}

static void add_retry_gap_sample(
    enum ssl_retry_gap_kind kind,
    const SSL *ssl,
    unsigned long long gap_ns,
    pthread_t want_thread,
    pthread_t retry_thread,
    int same_thread)
{
    unsigned long long gap_us = gap_ns / 1000ull;
    add_counter(&ssl_retry_gap_samples[kind], 1);
    update_max(&ssl_retry_gap_max_us[kind], gap_us);
    update_retry_gap_sample(
        &ssl_retry_gap_max_sample[kind], ssl, gap_us, want_thread, retry_thread, same_thread);
    add_hist(retry_gap_hist(kind), (size_t)gap_us);
    if (same_thread) {
        add_counter(&ssl_retry_gap_same_thread_samples[kind], 1);
        update_max(&ssl_retry_gap_same_thread_max_us[kind], gap_us);
    } else {
        add_counter(&ssl_retry_gap_migrated_samples[kind], 1);
        update_max(&ssl_retry_gap_migrated_max_us[kind], gap_us);
    }
}

static void add_want_write_retry_gap_sample(
    const SSL *ssl,
    unsigned long long gap_ns,
    pthread_t want_thread,
    pthread_t retry_thread,
    int same_thread)
{
    unsigned long long gap_us = gap_ns / 1000ull;
    add_counter(&ssl_retry_gap_want_write_samples, 1);
    update_max(&ssl_retry_gap_want_write_max_us, gap_us);
    update_retry_gap_sample(
        &ssl_retry_gap_want_write_max_sample, ssl, gap_us, want_thread, retry_thread, same_thread);
    if (same_thread) {
        add_counter(&ssl_retry_gap_want_write_same_thread_samples, 1);
        update_max(&ssl_retry_gap_want_write_same_thread_max_us, gap_us);
    } else {
        add_counter(&ssl_retry_gap_want_write_migrated_samples, 1);
        update_max(&ssl_retry_gap_want_write_migrated_max_us, gap_us);
    }
}

static struct ssl_retry_slot *retry_slot_locked(const SSL *ssl)
{
    uintptr_t base = ((uintptr_t)ssl >> 4) % SSL_TRACK_SLOTS;
    for (size_t offset = 0; offset < SSL_TRACK_SLOTS; offset++) {
        struct ssl_retry_slot *slot = &ssl_retry_slots[(base + offset) % SSL_TRACK_SLOTS];
        if (slot->ssl == ssl) {
            return slot;
        }
        if (slot->ssl == NULL) {
            slot->ssl = ssl;
            return slot;
        }
    }
    return NULL;
}

static void consume_ssl_retry_gap(const SSL *ssl, enum ssl_op_kind op, int depth)
{
    unsigned long long now = now_ns();
    if (now == 0 || ssl == NULL) {
        return;
    }

    pthread_mutex_lock(&ssl_retry_lock);
    struct ssl_retry_slot *slot = retry_slot_locked(ssl);
    if (slot != NULL) {
        if (slot->want_read_ns != 0 && now >= slot->want_read_ns) {
            pthread_t retry_thread = pthread_self();
            int same_thread = pthread_equal(slot->want_read_thread, retry_thread);
            add_retry_gap_sample(
                retry_gap_kind(op, depth),
                ssl,
                now - slot->want_read_ns,
                slot->want_read_thread,
                retry_thread,
                same_thread);
            slot->want_read_ns = 0;
        }
        if (slot->want_write_ns != 0 && now >= slot->want_write_ns) {
            pthread_t retry_thread = pthread_self();
            int same_thread = pthread_equal(slot->want_write_thread, retry_thread);
            add_want_write_retry_gap_sample(
                ssl, now - slot->want_write_ns, slot->want_write_thread, retry_thread, same_thread);
            slot->want_write_ns = 0;
        }
    }
    pthread_mutex_unlock(&ssl_retry_lock);
}

static void remember_ssl_want(const SSL *ssl, int code)
{
    unsigned long long now = now_ns();
    if (now == 0 || ssl == NULL) {
        return;
    }

    pthread_mutex_lock(&ssl_retry_lock);
    struct ssl_retry_slot *slot = retry_slot_locked(ssl);
    if (slot != NULL) {
        if (code == 2) {
            slot->want_read_ns = now;
            slot->want_read_thread = pthread_self();
        } else if (code == 3) {
            slot->want_write_ns = now;
            slot->want_write_thread = pthread_self();
        }
    }
    pthread_mutex_unlock(&ssl_retry_lock);
}

static void add_ssl_error_code(enum ssl_op_kind op, int code)
{
    unsigned int bin = ssl_error_bin(code);

    add_counter(&ssl_error_code_hist[bin], 1);
    switch (op) {
        case SSL_OP_READ:
        case SSL_OP_READ_EX:
            add_counter(&ssl_read_error_code_hist[bin], 1);
            break;
        case SSL_OP_WRITE:
        case SSL_OP_WRITE_EX:
            add_counter(&ssl_write_error_code_hist[bin], 1);
            break;
        case SSL_OP_CONNECT:
            add_counter(&ssl_connect_error_code_hist[bin], 1);
            break;
        case SSL_OP_SHUTDOWN:
            add_counter(&ssl_shutdown_error_code_hist[bin], 1);
            break;
        default:
            add_counter(&ssl_other_error_code_hist[bin], 1);
            break;
    }
}

int SSL_read(SSL *ssl, void *buf, int num)
{
    pthread_once(&resolve_once, resolve_symbols);
    int depth = ++tls_ssl_read_depth;
    consume_ssl_retry_gap(ssl, SSL_OP_READ, depth);
    int ret = real_ssl_read(ssl, buf, num);
    remember_ssl_op(SSL_OP_READ, ssl, ret);
    tls_ssl_read_depth--;

    add_counter(&ssl_read_calls, 1);
    if (num > 0) {
        add_counter(&ssl_read_requested_bytes, (unsigned long long)num);
        add_hist(ssl_read_requested_hist, (size_t)num);
    }
    if (ret > 0) {
        add_counter(&ssl_read_returned_bytes, (unsigned long long)ret);
        add_hist(ssl_read_returned_hist, (size_t)ret);
    } else {
        add_counter(&ssl_read_errors, 1);
    }
    return ret;
}

int SSL_read_ex(SSL *ssl, void *buf, size_t num, size_t *readbytes)
{
    pthread_once(&resolve_once, resolve_symbols);
    int depth = ++tls_ssl_read_depth;
    consume_ssl_retry_gap(ssl, SSL_OP_READ_EX, depth);
    int ret = real_ssl_read_ex(ssl, buf, num, readbytes);
    remember_ssl_op(SSL_OP_READ_EX, ssl, ret);
    tls_ssl_read_depth--;

    add_counter(&ssl_read_ex_calls, 1);
    add_counter(&ssl_read_requested_bytes, (unsigned long long)num);
    add_hist(ssl_read_requested_hist, num);
    if (ret == 1 && readbytes != NULL) {
        add_counter(&ssl_read_returned_bytes, (unsigned long long)*readbytes);
        add_hist(ssl_read_returned_hist, *readbytes);
    } else {
        add_counter(&ssl_read_errors, 1);
    }
    return ret;
}

int SSL_write(SSL *ssl, const void *buf, int num)
{
    pthread_once(&resolve_once, resolve_symbols);
    consume_ssl_retry_gap(ssl, SSL_OP_WRITE, 0);
    int ret = real_ssl_write(ssl, buf, num);
    remember_ssl_op(SSL_OP_WRITE, ssl, ret);

    add_counter(&ssl_write_calls, 1);
    if (num > 0) {
        add_counter(&ssl_write_requested_bytes, (unsigned long long)num);
        add_hist(ssl_write_requested_hist, (size_t)num);
    }
    if (ret > 0) {
        add_counter(&ssl_write_returned_bytes, (unsigned long long)ret);
        add_hist(ssl_write_returned_hist, (size_t)ret);
    } else {
        add_counter(&ssl_write_errors, 1);
    }
    return ret;
}

int SSL_write_ex(SSL *ssl, const void *buf, size_t num, size_t *written)
{
    pthread_once(&resolve_once, resolve_symbols);
    consume_ssl_retry_gap(ssl, SSL_OP_WRITE_EX, 0);
    int ret = real_ssl_write_ex(ssl, buf, num, written);
    remember_ssl_op(SSL_OP_WRITE_EX, ssl, ret);

    add_counter(&ssl_write_ex_calls, 1);
    add_counter(&ssl_write_requested_bytes, (unsigned long long)num);
    add_hist(ssl_write_requested_hist, num);
    if (ret == 1 && written != NULL) {
        add_counter(&ssl_write_returned_bytes, (unsigned long long)*written);
        add_hist(ssl_write_returned_hist, *written);
    } else {
        add_counter(&ssl_write_errors, 1);
    }
    return ret;
}

int SSL_connect(SSL *ssl)
{
    pthread_once(&resolve_once, resolve_symbols);
    consume_ssl_retry_gap(ssl, SSL_OP_CONNECT, 0);
    int ret = real_ssl_connect(ssl);
    remember_ssl_op(SSL_OP_CONNECT, ssl, ret);

    add_counter(&ssl_connect_calls, 1);
    if (ret <= 0) {
        add_counter(&ssl_connect_errors, 1);
    }
    return ret;
}

int SSL_shutdown(SSL *ssl)
{
    pthread_once(&resolve_once, resolve_symbols);
    consume_ssl_retry_gap(ssl, SSL_OP_SHUTDOWN, 0);
    int ret = real_ssl_shutdown(ssl);
    remember_ssl_op(SSL_OP_SHUTDOWN, ssl, ret);

    add_counter(&ssl_shutdown_calls, 1);
    if (ret < 0) {
        add_counter(&ssl_shutdown_errors, 1);
    }
    return ret;
}

int SSL_get_error(const SSL *ssl, int ret)
{
    pthread_once(&resolve_once, resolve_symbols);
    int code = real_ssl_get_error(ssl, ret);
    enum ssl_op_kind op = SSL_OP_OTHER;

    if (tls_last_ssl == ssl && tls_last_ret == ret) {
        op = tls_last_op;
    }

    add_counter(&ssl_get_error_calls, 1);
    add_ssl_error_code(op, code);
    if (code == 2 || code == 3) {
        remember_ssl_want(ssl, code);
    }
    return code;
}

int BIO_read(BIO *bio, void *buf, int len)
{
    pthread_once(&resolve_once, resolve_symbols);
    int ret = real_bio_read(bio, buf, len);

    add_counter(&bio_read_calls, 1);
    if (len > 0) {
        add_counter(&bio_read_requested_bytes, (unsigned long long)len);
        add_hist(bio_read_requested_hist, (size_t)len);
    }
    if (ret > 0) {
        add_counter(&bio_read_returned_bytes, (unsigned long long)ret);
        add_hist(bio_read_returned_hist, (size_t)ret);
    } else {
        add_counter(&bio_read_errors, 1);
    }
    return ret;
}

int BIO_write(BIO *bio, const void *buf, int len)
{
    pthread_once(&resolve_once, resolve_symbols);
    int ret = real_bio_write(bio, buf, len);

    add_counter(&bio_write_calls, 1);
    if (len > 0) {
        add_counter(&bio_write_requested_bytes, (unsigned long long)len);
        add_hist(bio_write_requested_hist, (size_t)len);
    }
    if (ret > 0) {
        add_counter(&bio_write_returned_bytes, (unsigned long long)ret);
        add_hist(bio_write_returned_hist, (size_t)ret);
    } else {
        add_counter(&bio_write_errors, 1);
    }
    return ret;
}

static unsigned long long load_counter(unsigned long long *counter)
{
    return __atomic_load_n(counter, __ATOMIC_RELAXED);
}

static void print_hist_len(FILE *out, const char *name, unsigned long long *hist, size_t len)
{
    fprintf(out, "\"%s\":[", name);
    for (size_t i = 0; i < len; i++) {
        if (i != 0) {
            fputc(',', out);
        }
        fprintf(out, "%llu", load_counter(&hist[i]));
    }
    fputc(']', out);
}

static void print_hist(FILE *out, const char *name, unsigned long long hist[HIST_BINS])
{
    print_hist_len(out, name, hist, HIST_BINS);
}

static void print_ssl_error_hist(FILE *out, const char *name, unsigned long long hist[SSL_ERROR_BINS])
{
    print_hist_len(out, name, hist, SSL_ERROR_BINS);
}

static void print_retry_gap_sample(FILE *out, const char *prefix, const struct ssl_retry_gap_sample *sample)
{
    fprintf(out, "\"%s_max_ssl\":\"0x%" PRIxPTR "\",", prefix, (uintptr_t)sample->ssl);
    fprintf(out, "\"%s_max_want_thread\":%llu,", prefix, sample->want_thread);
    fprintf(out, "\"%s_max_retry_thread\":%llu,", prefix, sample->retry_thread);
    fprintf(out, "\"%s_max_same_thread\":%s,", prefix, sample->same_thread ? "true" : "false");
}

static void print_retry_gap_stats(FILE *out, const char *prefix, enum ssl_retry_gap_kind kind)
{
    fprintf(out, "\"%s_samples\":%llu,", prefix, load_counter(&ssl_retry_gap_samples[kind]));
    fprintf(out, "\"%s_max_us\":%llu,", prefix, load_counter(&ssl_retry_gap_max_us[kind]));
    print_retry_gap_sample(out, prefix, &ssl_retry_gap_max_sample[kind]);
    fprintf(out, "\"%s_same_thread_samples\":%llu,", prefix,
            load_counter(&ssl_retry_gap_same_thread_samples[kind]));
    fprintf(out, "\"%s_same_thread_max_us\":%llu,", prefix,
            load_counter(&ssl_retry_gap_same_thread_max_us[kind]));
    fprintf(out, "\"%s_migrated_samples\":%llu,", prefix,
            load_counter(&ssl_retry_gap_migrated_samples[kind]));
    fprintf(out, "\"%s_migrated_max_us\":%llu,", prefix,
            load_counter(&ssl_retry_gap_migrated_max_us[kind]));
}

static void print_want_write_retry_gap_stats(FILE *out)
{
    fprintf(out, "\"ssl_retry_gap_want_write_samples\":%llu,",
            load_counter(&ssl_retry_gap_want_write_samples));
    fprintf(out, "\"ssl_retry_gap_want_write_max_us\":%llu,",
            load_counter(&ssl_retry_gap_want_write_max_us));
    print_retry_gap_sample(out, "ssl_retry_gap_want_write", &ssl_retry_gap_want_write_max_sample);
    fprintf(out, "\"ssl_retry_gap_want_write_same_thread_samples\":%llu,",
            load_counter(&ssl_retry_gap_want_write_same_thread_samples));
    fprintf(out, "\"ssl_retry_gap_want_write_same_thread_max_us\":%llu,",
            load_counter(&ssl_retry_gap_want_write_same_thread_max_us));
    fprintf(out, "\"ssl_retry_gap_want_write_migrated_samples\":%llu,",
            load_counter(&ssl_retry_gap_want_write_migrated_samples));
    fprintf(out, "\"ssl_retry_gap_want_write_migrated_max_us\":%llu,",
            load_counter(&ssl_retry_gap_want_write_migrated_max_us));
}

static void print_report(void)
{
    const char *path = getenv("YLONG_SSL_TRACE_FILE");
    const char *label = getenv("YLONG_SSL_TRACE_LABEL");
    FILE *out = stderr;
    if (path != NULL && path[0] != '\0') {
        out = fopen(path, "a");
        if (out == NULL) {
            out = stderr;
        }
    }

    fprintf(out, "{\"kind\":\"ssl_trace\",\"label\":\"%s\",", label == NULL ? "" : label);
    fprintf(out, "\"ssl_read_calls\":%llu,", load_counter(&ssl_read_calls));
    fprintf(out, "\"ssl_read_ex_calls\":%llu,", load_counter(&ssl_read_ex_calls));
    fprintf(out, "\"ssl_read_errors\":%llu,", load_counter(&ssl_read_errors));
    fprintf(out, "\"ssl_read_requested_bytes\":%llu,", load_counter(&ssl_read_requested_bytes));
    fprintf(out, "\"ssl_read_returned_bytes\":%llu,", load_counter(&ssl_read_returned_bytes));
    fprintf(out, "\"ssl_write_calls\":%llu,", load_counter(&ssl_write_calls));
    fprintf(out, "\"ssl_write_ex_calls\":%llu,", load_counter(&ssl_write_ex_calls));
    fprintf(out, "\"ssl_write_errors\":%llu,", load_counter(&ssl_write_errors));
    fprintf(out, "\"ssl_write_requested_bytes\":%llu,", load_counter(&ssl_write_requested_bytes));
    fprintf(out, "\"ssl_write_returned_bytes\":%llu,", load_counter(&ssl_write_returned_bytes));
    fprintf(out, "\"ssl_connect_calls\":%llu,", load_counter(&ssl_connect_calls));
    fprintf(out, "\"ssl_connect_errors\":%llu,", load_counter(&ssl_connect_errors));
    fprintf(out, "\"ssl_shutdown_calls\":%llu,", load_counter(&ssl_shutdown_calls));
    fprintf(out, "\"ssl_shutdown_errors\":%llu,", load_counter(&ssl_shutdown_errors));
    fprintf(out, "\"ssl_get_error_calls\":%llu,", load_counter(&ssl_get_error_calls));
    print_retry_gap_stats(out, "ssl_retry_gap_read_depth1", SSL_RETRY_GAP_READ_DEPTH1);
    print_retry_gap_stats(out, "ssl_retry_gap_read_depth2", SSL_RETRY_GAP_READ_DEPTH2);
    print_retry_gap_stats(out, "ssl_retry_gap_read_depth_other", SSL_RETRY_GAP_READ_DEPTH_OTHER);
    print_retry_gap_stats(out, "ssl_retry_gap_connect", SSL_RETRY_GAP_CONNECT);
    print_retry_gap_stats(out, "ssl_retry_gap_other", SSL_RETRY_GAP_OTHER);
    print_want_write_retry_gap_stats(out);
    fprintf(out, "\"bio_read_calls\":%llu,", load_counter(&bio_read_calls));
    fprintf(out, "\"bio_read_errors\":%llu,", load_counter(&bio_read_errors));
    fprintf(out, "\"bio_read_requested_bytes\":%llu,", load_counter(&bio_read_requested_bytes));
    fprintf(out, "\"bio_read_returned_bytes\":%llu,", load_counter(&bio_read_returned_bytes));
    fprintf(out, "\"bio_write_calls\":%llu,", load_counter(&bio_write_calls));
    fprintf(out, "\"bio_write_errors\":%llu,", load_counter(&bio_write_errors));
    fprintf(out, "\"bio_write_requested_bytes\":%llu,", load_counter(&bio_write_requested_bytes));
    fprintf(out, "\"bio_write_returned_bytes\":%llu,", load_counter(&bio_write_returned_bytes));
    print_hist(out, "ssl_read_requested_hist", ssl_read_requested_hist);
    fputc(',', out);
    print_hist(out, "ssl_read_returned_hist", ssl_read_returned_hist);
    fputc(',', out);
    print_hist(out, "ssl_write_requested_hist", ssl_write_requested_hist);
    fputc(',', out);
    print_hist(out, "ssl_write_returned_hist", ssl_write_returned_hist);
    fputc(',', out);
    print_hist(out, "bio_read_requested_hist", bio_read_requested_hist);
    fputc(',', out);
    print_hist(out, "bio_read_returned_hist", bio_read_returned_hist);
    fputc(',', out);
    print_hist(out, "bio_write_requested_hist", bio_write_requested_hist);
    fputc(',', out);
    print_hist(out, "bio_write_returned_hist", bio_write_returned_hist);
    fputc(',', out);
    print_ssl_error_hist(out, "ssl_error_code_hist", ssl_error_code_hist);
    fputc(',', out);
    print_ssl_error_hist(out, "ssl_read_error_code_hist", ssl_read_error_code_hist);
    fputc(',', out);
    print_ssl_error_hist(out, "ssl_write_error_code_hist", ssl_write_error_code_hist);
    fputc(',', out);
    print_ssl_error_hist(out, "ssl_connect_error_code_hist", ssl_connect_error_code_hist);
    fputc(',', out);
    print_ssl_error_hist(out, "ssl_shutdown_error_code_hist", ssl_shutdown_error_code_hist);
    fputc(',', out);
    print_ssl_error_hist(out, "ssl_other_error_code_hist", ssl_other_error_code_hist);
    fputc(',', out);
    print_hist_len(out, "ssl_retry_gap_read_depth1_hist_us", ssl_retry_gap_read_depth1_hist, HIST_BINS);
    fputc(',', out);
    print_hist_len(out, "ssl_retry_gap_read_depth2_hist_us", ssl_retry_gap_read_depth2_hist, HIST_BINS);
    fputc(',', out);
    print_hist_len(
        out, "ssl_retry_gap_read_depth_other_hist_us", ssl_retry_gap_read_depth_other_hist, HIST_BINS);
    fputc(',', out);
    print_hist_len(out, "ssl_retry_gap_connect_hist_us", ssl_retry_gap_connect_hist, HIST_BINS);
    fputc(',', out);
    print_hist_len(out, "ssl_retry_gap_other_hist_us", ssl_retry_gap_other_hist, HIST_BINS);
    fprintf(out, "}\n");

    if (out != stderr) {
        fclose(out);
    }
}

__attribute__((destructor)) static void ssl_trace_fini(void)
{
    print_report();
}
