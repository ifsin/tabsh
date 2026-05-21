/* pty_unix.c -- forkpty-based PTY backend (Linux, macOS, *BSD) */
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include <sys/ioctl.h>
#include <sys/wait.h>

#if defined(__OpenBSD__) || defined(__APPLE__)
#include <util.h>
#elif defined(__FreeBSD__)
#include <libutil.h>
#else
#include <pty.h>
#endif

#include "pty.h"
#include "utils.h"

struct pty_impl_s {
    int         pty_fd;
    uv_thread_t tid;
};

static void async_free_cb(uv_handle_t *handle) {
    free((uv_async_t *)handle->data);
}

void pty_impl_destroy(pty_process *process) {
    pty_impl_t *impl = process->impl;
    if (impl == NULL) return;
    if (impl->pty_fd >= 0) close(impl->pty_fd);
    uv_thread_join(&impl->tid);
    free(impl);
    process->impl = NULL;
}

bool pty_resize(pty_process *process) {
    if (process == NULL || process->impl == NULL) return false;
    if (process->columns <= 0 || process->rows <= 0) return false;
    struct winsize size = { process->rows, process->columns, 0, 0 };
    return ioctl(process->impl->pty_fd, TIOCSWINSZ, &size) == 0;
}

bool pty_kill(pty_process *process, int sig) {
    if (process == NULL) return false;
    return uv_kill(-process->pid, sig) == 0;
}

pid_t pty_get_fg_pid(pty_process *process) {
    if (process == NULL || process->impl == NULL) return -1;
    return tcgetpgrp(process->impl->pty_fd);
}

static bool fd_set_cloexec(const int fd) {
    int flags = fcntl(fd, F_GETFD);
    if (flags < 0) return false;
    return (flags & FD_CLOEXEC) == 0 || fcntl(fd, F_SETFD, flags | FD_CLOEXEC) != -1;
}

static bool fd_duplicate(int fd, uv_pipe_t *pipe) {
    int fd_dup = dup(fd);
    if (fd_dup < 0) return false;
    if (!fd_set_cloexec(fd_dup)) return false;
    int status = uv_pipe_open(pipe, fd_dup);
    if (status) close(fd_dup);
    return status == 0;
}

static void wait_cb(void *arg) {
    pty_process *process = (pty_process *)arg;
    pid_t        pid;
    int          stat;
    do
        pid = waitpid(process->pid, &stat, 0);
    while (pid != process->pid && errno == EINTR);

    if (WIFEXITED(stat)) {
        process->exit_code = WEXITSTATUS(stat);
    }
    if (WIFSIGNALED(stat)) {
        int sig = WTERMSIG(stat);
        process->exit_code   = 128 + sig;
        process->exit_signal = sig;
    }

    uv_async_send(&process->async);
}

static void async_cb(uv_async_t *async) {
    pty_process *process = (pty_process *)async->data;
    process->exit_cb(process);
    uv_close((uv_handle_t *)async, async_free_cb);
    process_free(process);
}

int pty_spawn(pty_process *process, pty_read_cb read_cb, pty_exit_cb exit_cb) {
    int status = 0;
    uv_disable_stdio_inheritance();

    int            master;
    pid_t          pid;
    struct winsize size = { process->rows, process->columns, 0, 0 };
    pid = forkpty(&master, NULL, NULL, &size);
    if (pid < 0) {
        return -errno;
    } else if (pid == 0) {
        setsid();
        if (process->cwd != NULL) chdir(process->cwd);
        if (process->envp != NULL) {
            for (char **p = process->envp; *p; p++) putenv(*p);
        }
        int ret = execvp(process->argv[0], process->argv);
        if (ret < 0) {
            perror("execvp failed\n");
            _exit(-errno);
        }
    }

    int flags = fcntl(master, F_GETFL);
    if (flags == -1) { status = -errno; goto error; }
    if (fcntl(master, F_SETFL, flags | O_NONBLOCK) == -1) { status = -errno; goto error; }
    if (!fd_set_cloexec(master)) { status = -errno; goto error; }

    process->in  = xmalloc(sizeof(uv_pipe_t));
    process->out = xmalloc(sizeof(uv_pipe_t));
    uv_pipe_init(process->loop, process->in, 0);
    uv_pipe_init(process->loop, process->out, 0);

    if (!fd_duplicate(master, process->in) || !fd_duplicate(master, process->out)) {
        status = -errno;
        goto error;
    }

    pty_impl_t *impl = xmalloc(sizeof(pty_impl_t));
    impl->pty_fd        = master;
    process->impl       = impl;
    process->pid        = pid;
    process->paused     = true;
    process->read_cb    = read_cb;
    process->exit_cb    = exit_cb;
    process->async.data = process;
    uv_async_init(process->loop, &process->async, async_cb);
    uv_thread_create(&impl->tid, wait_cb, process);

    return 0;

error:
    close(master);
    uv_kill(pid, SIGKILL);
    waitpid(pid, NULL, 0);
    return status;
}
