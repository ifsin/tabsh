#ifndef TTYD_PTY_H
#define TTYD_PTY_H

#include <stdbool.h>
#include <stdint.h>
#include <sys/types.h>
#include <uv.h>

typedef struct {
  char *base;
  size_t len;
} pty_buf_t;

typedef struct pty_impl_s pty_impl_t;  /* opaque, defined per platform */
struct pty_process_;
typedef struct pty_process_ pty_process;
typedef void (*pty_read_cb)(pty_process *, pty_buf_t *, bool);
typedef void (*pty_exit_cb)(pty_process *);

struct pty_process_ {
  int pid, exit_code, exit_signal;
  uint16_t columns, rows;

  char **argv;
  char **envp;
  char *cwd;

  uv_loop_t *loop;
  uv_async_t async;
  uv_pipe_t *in;
  uv_pipe_t *out;
  bool paused;

  pty_read_cb read_cb;
  pty_exit_cb exit_cb;
  void *ctx;

  pty_impl_t *impl;  /* platform-private state */
};

/* portable (pty_common.c) */
pty_buf_t *pty_buf_init(char *base, size_t len);
void pty_buf_free(pty_buf_t *buf);
pty_process *process_init(void *ctx, uv_loop_t *loop, char *argv[], char *envp[]);
bool process_running(pty_process *process);
void process_free(pty_process *process);
void pty_pause(pty_process *process);
void pty_resume(pty_process *process);
int pty_write(pty_process *process, pty_buf_t *buf);

/* platform-specific (pty_unix.c / pty_win32.c) */
int pty_spawn(pty_process *process, pty_read_cb read_cb, pty_exit_cb exit_cb);
bool pty_resize(pty_process *process);
bool pty_kill(pty_process *process, int sig);
pid_t pty_get_fg_pid(pty_process *process);  /* fg pgrp on Unix, process->pid on Win */

/* internal: implemented per platform, called by pty_common.c */
void pty_impl_destroy(pty_process *process);

#endif  // TTYD_PTY_H
