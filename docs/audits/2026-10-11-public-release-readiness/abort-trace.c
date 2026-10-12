#define _GNU_SOURCE
#include <dlfcn.h>
#include <execinfo.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <time.h>
static void trace_window(void) {
    struct timespec delay = {0, 200000000};
    nanosleep(&delay, NULL);
}
__attribute__((constructor)) static void prepare_trace(void) {
    void *frame[1];
    backtrace(frame, 1);
    /* Registered before main's lazy RocksDB globals: preserve a short window
       after their finalizers so a racing abort can finish writing its trace. */
    atexit(trace_window);
}
__attribute__((noreturn)) void abort(void) {
    void *frames[80];
    char name[32] = {0};
    pthread_getname_np(pthread_self(), name, sizeof(name));
    fprintf(stderr, "EEZ_ABORT_TRACE thread=%s pid=%d\n", name, getpid());
    int count = backtrace(frames, 80);
    backtrace_symbols_fd(frames, count, STDERR_FILENO);
    void (*real_abort)(void) = dlsym(RTLD_NEXT, "abort");
    real_abort();
    _exit(134);
}
