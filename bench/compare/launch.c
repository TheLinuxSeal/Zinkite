/* Runs a benchmark and reports its wall time and peak resident set, for run.py.
 *
 *   launch PROGRAM [ARGS...]
 *
 * The program's own output passes through; on exit this writes "<ms> <peak KB> <status>"
 * to stderr. It exists because a child that Python forks starts out sharing Python's
 * pages, and its peak resident set counts them; forked from this small program, it
 * counts almost nothing but its own. */
#define _GNU_SOURCE
#include <stdio.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc < 2) { fprintf(stderr, "usage: launch PROGRAM [ARGS...]\n"); return 2; }
    struct timespec t0, t1;
    clock_gettime(CLOCK_MONOTONIC, &t0);
    pid_t pid = fork();
    if (pid == 0) {
        int dn = fileno(fopen("/dev/null", "w"));
        dup2(dn, 2);
        execv(argv[1], argv + 1);
        _exit(127);
    }
    int status;
    struct rusage ru;
    wait4(pid, &status, 0, &ru);
    clock_gettime(CLOCK_MONOTONIC, &t1);
    double ms = (t1.tv_sec - t0.tv_sec) * 1e3 + (t1.tv_nsec - t0.tv_nsec) / 1e6;
    int code = WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
    fprintf(stderr, "%.3f %ld %d\n", ms, ru.ru_maxrss, code);
    return 0;
}
