// Copyright (c) 2026 Huawei Device Co., Ltd.
// Licensed under the Apache License, Version 2.0.
//
// LD_PRELOAD helper for HTTPS proxy benchmark diagnostics. It records OpenSSL
// read/write size histograms without requiring tracefs or bpftrace privileges.

#define _GNU_SOURCE

#include <dlfcn.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct ssl_st SSL;

typedef int (*ssl_read_fn)(SSL *, void *, int);
typedef int (*ssl_read_ex_fn)(SSL *, void *, size_t, size_t *);
typedef int (*ssl_write_fn)(SSL *, const void *, int);
typedef int (*ssl_write_ex_fn)(SSL *, const void *, size_t, size_t *);

#define HIST_BINS 65

static ssl_read_fn real_ssl_read;
static ssl_read_ex_fn real_ssl_read_ex;
static ssl_write_fn real_ssl_write;
static ssl_write_ex_fn real_ssl_write_ex;
static pthread_once_t resolve_once = PTHREAD_ONCE_INIT;

static unsigned long long ssl_read_calls;
static unsigned long long ssl_read_ex_calls;
static unsigned long long ssl_write_calls;
static unsigned long long ssl_write_ex_calls;
static unsigned long long ssl_read_errors;
static unsigned long long ssl_write_errors;
static unsigned long long ssl_read_requested_bytes;
static unsigned long long ssl_read_returned_bytes;
static unsigned long long ssl_write_requested_bytes;
static unsigned long long ssl_write_returned_bytes;
static unsigned long long ssl_read_requested_hist[HIST_BINS];
static unsigned long long ssl_read_returned_hist[HIST_BINS];
static unsigned long long ssl_write_requested_hist[HIST_BINS];
static unsigned long long ssl_write_returned_hist[HIST_BINS];

static void resolve_symbols(void)
{
    real_ssl_read = (ssl_read_fn)dlsym(RTLD_NEXT, "SSL_read");
    real_ssl_read_ex = (ssl_read_ex_fn)dlsym(RTLD_NEXT, "SSL_read_ex");
    real_ssl_write = (ssl_write_fn)dlsym(RTLD_NEXT, "SSL_write");
    real_ssl_write_ex = (ssl_write_ex_fn)dlsym(RTLD_NEXT, "SSL_write_ex");
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

int SSL_read(SSL *ssl, void *buf, int num)
{
    pthread_once(&resolve_once, resolve_symbols);
    int ret = real_ssl_read(ssl, buf, num);

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
    int ret = real_ssl_read_ex(ssl, buf, num, readbytes);

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
    int ret = real_ssl_write(ssl, buf, num);

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
    int ret = real_ssl_write_ex(ssl, buf, num, written);

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

static unsigned long long load_counter(unsigned long long *counter)
{
    return __atomic_load_n(counter, __ATOMIC_RELAXED);
}

static void print_hist(FILE *out, const char *name, unsigned long long hist[HIST_BINS])
{
    fprintf(out, "\"%s\":[", name);
    for (size_t i = 0; i < HIST_BINS; i++) {
        if (i != 0) {
            fputc(',', out);
        }
        fprintf(out, "%llu", load_counter(&hist[i]));
    }
    fputc(']', out);
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
    print_hist(out, "ssl_read_requested_hist", ssl_read_requested_hist);
    fputc(',', out);
    print_hist(out, "ssl_read_returned_hist", ssl_read_returned_hist);
    fputc(',', out);
    print_hist(out, "ssl_write_requested_hist", ssl_write_requested_hist);
    fputc(',', out);
    print_hist(out, "ssl_write_returned_hist", ssl_write_returned_hist);
    fprintf(out, "}\n");

    if (out != stderr) {
        fclose(out);
    }
}

__attribute__((destructor)) static void ssl_trace_fini(void)
{
    print_report();
}
