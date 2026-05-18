/* pty_common.c -- portable bits shared by pty_unix.c and pty_win32.c */
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>

#include "pty.h"
#include "utils.h"

static void alloc_cb(uv_handle_t *unused, size_t suggested_size, uv_buf_t *buf) {
  buf->base = xmalloc(suggested_size);
  buf->len = suggested_size;
}

static void read_cb(uv_stream_t *stream, ssize_t n, const uv_buf_t *buf) {
  uv_read_stop(stream);
  pty_process *process = (pty_process *)stream->data;
  if (n <= 0) {
    if (n == UV_ENOBUFS || n == 0) goto done;
    process->read_cb(process, NULL, true);
    goto done;
  }
  process->read_cb(process, pty_buf_init(buf->base, (size_t)n), false);

done:
  free(buf->base);
}

static void write_cb(uv_write_t *req, int unused) {
  pty_buf_t *buf = (pty_buf_t *)req->data;
  pty_buf_free(buf);
  free(req);
}

static void close_cb(uv_handle_t *handle) { free(handle); }

pty_buf_t *pty_buf_init(char *base, size_t len) {
  pty_buf_t *buf = xmalloc(sizeof(pty_buf_t));
  buf->base = xmalloc(len);
  memcpy(buf->base, base, len);
  buf->len = len;
  return buf;
}

void pty_buf_free(pty_buf_t *buf) {
  if (buf == NULL) return;
  if (buf->base != NULL) free(buf->base);
  free(buf);
}

pty_process *process_init(void *ctx, uv_loop_t *loop, char *argv[], char *envp[]) {
  pty_process *process = xmalloc(sizeof(pty_process));
  memset(process, 0, sizeof(pty_process));
  process->ctx = ctx;
  process->loop = loop;
  process->argv = argv;
  process->envp = envp;
  process->columns = 80;
  process->rows = 24;
  process->exit_code = -1;
  return process;
}

bool process_running(pty_process *process) {
  return process != NULL && process->pid > 0 && uv_kill(process->pid, 0) == 0;
}

void process_free(pty_process *process) {
  if (process == NULL) return;
  pty_impl_destroy(process);
  if (process->in != NULL) uv_close((uv_handle_t *)process->in, close_cb);
  if (process->out != NULL) uv_close((uv_handle_t *)process->out, close_cb);
  if (process->argv != NULL) free(process->argv);
  if (process->cwd != NULL) free(process->cwd);
  if (process->envp != NULL) {
    for (char **p = process->envp; *p; p++) free(*p);
    free(process->envp);
  }
}

void pty_pause(pty_process *process) {
  if (process == NULL) return;
  if (process->paused) return;
  uv_read_stop((uv_stream_t *)process->out);
}

void pty_resume(pty_process *process) {
  if (process == NULL) return;
  if (!process->paused) return;
  process->out->data = process;
  uv_read_start((uv_stream_t *)process->out, alloc_cb, read_cb);
}

int pty_write(pty_process *process, pty_buf_t *buf) {
  if (process == NULL) {
    pty_buf_free(buf);
    return UV_ESRCH;
  }
  uv_buf_t b = uv_buf_init(buf->base, buf->len);
  uv_write_t *req = xmalloc(sizeof(uv_write_t));
  req->data = buf;
  return uv_write(req, (uv_stream_t *)process->in, &b, 1, write_cb);
}
