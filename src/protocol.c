#include <ctype.h>
#include <errno.h>
#include <json.h>
#include <libwebsockets.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifndef _WIN32
#include <fcntl.h>
#include <termios.h>
#include <unistd.h>
#ifdef __APPLE__
#include <sys/sysctl.h>
#endif
#endif

#include "notify.h"
#include "pty.h"
#include "server.h"
#include "terminal.h"
#include "utils.h"

// initial message list
static char initial_cmds[] = {PREFERENCES};

#include "favicon.h"

#ifndef _WIN32
static void append_arg_text(char** out, size_t* out_len, const char* text, size_t text_len) {
  if (*out_len <= 1) return;

  size_t len = text_len < *out_len - 1 ? text_len : *out_len - 1;
  memcpy(*out, text, len);
  *out += len;
  *out_len -= len;
  **out = '\0';
}

static bool shell_arg_is_safe(const char* arg) {
  if (*arg == '\0') return false;

  for (const unsigned char* p = (const unsigned char*)arg; *p != '\0'; p++) {
    if (isalnum(*p) || strchr("_+-./:=,@%", *p) != NULL) continue;
    return false;
  }

  return true;
}

static const char* detect_shell(const char* command) {
  static const char* shell_names[] = {"zsh",  "bash", "fish",       "sh",  "dash", "ksh",
                                      "tcsh", "pwsh", "powershell", "cmd", NULL};
  const char* argv0 = strrchr(command, '/');
  argv0 = argv0 != NULL ? argv0 + 1 : command;

  char name[64] = {0};
  const char* sp = strchr(argv0, ' ');
  size_t len = sp ? (size_t)(sp - argv0) : strlen(argv0);
  if (len >= sizeof(name)) return NULL;
  strncpy(name, argv0, len);

  for (int i = 0; shell_names[i] != NULL; i++) {
    if (strcmp(name, shell_names[i]) == 0) return shell_names[i];
  }
  return NULL;
}

static bool command_is_shell(const char* command) { return detect_shell(command) != NULL; }

static void append_shell_arg(char** out, size_t* out_len, const char* arg) {
  if (shell_arg_is_safe(arg)) {
    append_arg_text(out, out_len, arg, strlen(arg));
    return;
  }

  append_arg_text(out, out_len, "'", 1);
  for (const char* p = arg; *p != '\0'; p++) {
    if (*p == '\'') {
      append_arg_text(out, out_len, "'\\''", 4);
    } else {
      append_arg_text(out, out_len, p, 1);
    }
  }
  append_arg_text(out, out_len, "'", 1);
}

static void append_process_arg(char** out, size_t* out_len, const char* arg, bool shell_quote) {
  if (shell_quote)
    append_shell_arg(out, out_len, arg);
  else
    append_arg_text(out, out_len, arg, strlen(arg));
}

static void get_process_argv(pid_t pid, char* out, size_t out_len, bool shell_quote) {
  out[0] = '\0';
#ifdef __APPLE__
  int mib[3] = {CTL_KERN, KERN_PROCARGS2, pid};
  char buf[8192];
  size_t buf_size = sizeof(buf);
  if (sysctl(mib, 3, buf, &buf_size, NULL, 0) != 0) return;
  int argc = *(int*)buf;
  char* p = buf + sizeof(int);
  char* end = buf + buf_size;
  p += strnlen(p, (size_t)(end - p)) + 1;
  while (p < end && *p == '\0') p++;
  char* dst = out;
  size_t remaining = out_len;
  for (int i = 0; i < argc && p < end && remaining > 1; i++) {
    if (i > 0) append_arg_text(&dst, &remaining, " ", 1);
    append_process_arg(&dst, &remaining, p, shell_quote);
    p += strnlen(p, (size_t)(end - p)) + 1;
  }
#else
  char path[64];
  snprintf(path, sizeof(path), "/proc/%d/cmdline", (int)pid);
  int fd = open(path, O_RDONLY);
  if (fd < 0) return;
  char buf[8192];
  ssize_t n = read(fd, buf, sizeof(buf) - 1);
  close(fd);
  if (n <= 0) return;

  char* dst = out;
  size_t remaining = out_len;
  for (ssize_t i = 0; i < n && remaining > 1;) {
    size_t arg_len = strnlen(buf + i, (size_t)(n - i));
    if (dst != out) append_arg_text(&dst, &remaining, " ", 1);
    append_process_arg(&dst, &remaining, buf + i, shell_quote);
    i += (ssize_t)arg_len + 1;
  }
#endif
}
#endif

static void reattach_sigwinch_cb(uv_timer_t* timer) {
  pty_process* process = (pty_process*)timer->data;
#ifndef _WIN32
  if (process != NULL && process->pid > 0) {
    pty_resize(process);
    uv_kill(-process->pid, SIGWINCH);
  }
#endif
  uv_close((uv_handle_t*)timer, (uv_close_cb)free);
}

static int send_initial_message(struct lws* wsi, int index) {
  unsigned char message[LWS_PRE + 1 + 4096];
  unsigned char* p = &message[LWS_PRE];
  int n = 0;

  char cmd = initial_cmds[index];
  switch (cmd) {
    case PREFERENCES:
      n = snprintf((char*)p, 1 + 4096, "%c%s", cmd, server->prefs_json);
      break;
    default:
      break;
  }

  return lws_write(wsi, p, (size_t)n, LWS_WRITE_BINARY);
}

static json_object* parse_window_size(const char* buf, size_t len, uint16_t* cols, uint16_t* rows) {
  json_tokener* tok = json_tokener_new();
  json_object* obj = json_tokener_parse_ex(tok, buf, len);
  struct json_object* o = NULL;

  if (json_object_object_get_ex(obj, "columns", &o)) *cols = (uint16_t)json_object_get_int(o);
  if (json_object_object_get_ex(obj, "rows", &o)) *rows = (uint16_t)json_object_get_int(o);

  json_tokener_free(tok);
  return obj;
}

static bool check_host_origin(struct lws* wsi) {
  char buf[256];
  memset(buf, 0, sizeof(buf));
  int len = lws_hdr_copy(wsi, buf, (int)sizeof(buf), WSI_TOKEN_ORIGIN);
  if (len <= 0) return false;

  const char *prot, *address, *path;
  int port;
  if (lws_parse_uri(buf, &prot, &address, &port, &path)) return false;
  if (port == 80 || port == 443) {
    snprintf(buf, sizeof(buf), "%s", address);
  } else {
    snprintf(buf, sizeof(buf), "%s:%d", address, port);
  }

  char host_buf[256];
  memset(host_buf, 0, sizeof(host_buf));
  len = lws_hdr_copy(wsi, host_buf, (int)sizeof(host_buf), WSI_TOKEN_HOST);

  return len > 0 && strcasecmp(buf, host_buf) == 0;
}

#ifndef _WIN32
static void check_foreground_process(struct pss_tty* pss) {
  pid_t fgpid = pss->process ? pty_get_fg_pid(pss->process) : -1;
  if (fgpid <= 0) return;
  if (fgpid == pss->last_fgpid) return;
  pss->last_fgpid = fgpid;

  char raw_argv[APP_COMMAND_LEN] = {0};
  char quoted_argv[APP_COMMAND_LEN] = {0};
  get_process_argv(fgpid, raw_argv, sizeof(raw_argv), false);
  get_process_argv(fgpid, quoted_argv, sizeof(quoted_argv), true);

  if ((pss->session != NULL && fgpid == pss->session->root_pid) || command_is_shell(raw_argv)) {
    if (pss->current_app[0] != '\0') {
      pss->current_app[0] = '\0';
      pss->pending_app[0] = '\0';
      pss->pending_app_send = true;
      lws_callback_on_writable(pss->wsi);
    }
  } else if (strcmp(quoted_argv, pss->current_app) != 0) {
    strncpy(pss->current_app, quoted_argv, sizeof(pss->current_app) - 1);
    pss->current_app[sizeof(pss->current_app) - 1] = '\0';
    strncpy(pss->pending_app, quoted_argv, sizeof(pss->pending_app) - 1);
    pss->pending_app[sizeof(pss->pending_app) - 1] = '\0';
    pss->pending_app_send = true;
    lws_callback_on_writable(pss->wsi);
  }

  char formula[256] = {0};
  bool is_brew = raw_argv[0] != '\0' && favicon_resolve_formula(raw_argv, formula, sizeof(formula));

  if (!is_brew) {
    if (pss->current_favicon_formula[0] != '\0') {
      pss->current_favicon_formula[0] = '\0';
      pss->pending_favicon[0] = '\0';
      pss->pending_favicon_send = true;
      lws_callback_on_writable(pss->wsi);
    }
  } else if (strcmp(formula, pss->current_favicon_formula) != 0) {
    strncpy(pss->current_favicon_formula, formula, sizeof(pss->current_favicon_formula) - 1);
    pss->current_favicon_formula[sizeof(pss->current_favicon_formula) - 1] = '\0';

    char cache_path[512], none_path[512];
    snprintf(cache_path, sizeof(cache_path), "/tmp/ttyd-favicons/%s.png", formula);
    snprintf(none_path, sizeof(none_path), "/tmp/ttyd-favicons/%s.none", formula);

    if (access(none_path, F_OK) == 0) {
      // cached none
    } else if (access(cache_path, F_OK) == 0) {
      snprintf(pss->pending_favicon, sizeof(pss->pending_favicon), "%s%s.png", endpoints.favicon, formula);
      pss->pending_favicon_send = true;
      lws_callback_on_writable(pss->wsi);
    } else {
      favicon_queue_fetch(pss, formula, cache_path, none_path);
    }
  }
}
#endif

static void pty_ring_write(const char* data, size_t len) {
  if (len == 0 || len >= PTY_RING_SIZE) return;
  size_t space = PTY_RING_SIZE - server->pty_ring_head;
  if (len <= space) {
    memcpy(server->pty_ring + server->pty_ring_head, data, len);
  } else {
    memcpy(server->pty_ring + server->pty_ring_head, data, space);
    memcpy(server->pty_ring, data + space, len - space);
  }
  server->pty_ring_head = (server->pty_ring_head + len) % PTY_RING_SIZE;
  server->pty_ring_len = server->pty_ring_len + len > PTY_RING_SIZE ? PTY_RING_SIZE : server->pty_ring_len + len;
}

static void process_read_cb(pty_process* process, pty_buf_t* buf, bool eof) {
  session_t* session = (session_t*)process->ctx;
  if (session->detached) {
    pty_buf_free(buf);
    return;
  }

  if (eof && !process_running(process)) {
    session->pss->lws_close_status = process->exit_code == 0 ? 1000 : 1006;
  } else {
#ifndef _WIN32
    check_foreground_process(session->pss);
#endif
    pty_ring_write(buf->base, buf->len);
    if (session->terminal != NULL) {
      bool changed = terminal_push(session->terminal, buf->base, buf->len);
      pty_buf_free(buf);
      if (changed) session->pss->pending_frame = true;
      pty_resume(process);
    }
  }
  lws_callback_on_writable(session->pss->wsi);
}

static void process_exit_cb(pty_process* process) {
  session_t* session = (session_t*)process->ctx;
  struct pss_tty* pss = session->pss;
  if (session->detached) {
    lwsl_notice("process killed with signal %d, pid: %d\n", process->exit_signal, process->pid);
    if (pss != NULL) pss->session = NULL;
    goto done;
  }

  lwsl_notice("process exited with code %d, pid: %d\n", process->exit_code, process->pid);
  session->pss->process = NULL;
  session->pss->session = NULL;
  session->pss->lws_close_status = process->exit_code == 0 ? 1000 : 1006;
  lws_callback_on_writable(session->pss->wsi);

done:
#ifndef _WIN32
  if (session->notify != NULL) {
    notify_ctx_destroy(session->notify);
    session->notify = NULL;
    if (pss != NULL) pss->notify = NULL;
  }
#endif
  session_remove(session);
  if (session->timer != NULL) {
    uv_timer_stop(session->timer);
    uv_close((uv_handle_t*)session->timer, (uv_close_cb)free);
  }
  if (session->terminal != NULL) {
    terminal_destroy(session->terminal);
    session->terminal = NULL;
  }
  free(session);

  // if we are going to exit, do it now.
  if (force_exit) exit(0);
}

static char** build_args(struct pss_tty* pss) {
  const char* shell = detect_shell(server->argv[0]);
  int extra = 0;

  if (shell != NULL && pss->notify != NULL) {
    if (strcmp(shell, "bash") == 0) {
      extra = 2;
    } else if (strcmp(shell, "pwsh") == 0 || strcmp(shell, "powershell") == 0) {
      extra = 3;
    } else if (strcmp(shell, "cmd") == 0) {
      extra = 2;
    }
  }

  int i, n = 0;
  char** argv = xmalloc((server->argc + extra + 1) * sizeof(char*));

  for (i = 0; i < server->argc; i++) {
    argv[n++] = server->argv[i];
  }

  if (extra > 0) {
    const char* init_dir = notify_init_dir(pss->notify);
    if (strcmp(shell, "bash") == 0) {
      argv[n++] = "--rcfile";
      char* path = xmalloc(strlen(init_dir) + 8);
      sprintf(path, "%s/bashrc", init_dir);
      argv[n++] = path;
    } else if (strcmp(shell, "pwsh") == 0 || strcmp(shell, "powershell") == 0) {
      argv[n++] = "-NoExit";
      argv[n++] = "-File";
      char* path = xmalloc(strlen(init_dir) + 10);
      sprintf(path, "%s/init.ps1", init_dir);
      argv[n++] = path;
    } else if (strcmp(shell, "cmd") == 0) {
      argv[n++] = "/K";
      char* path = xmalloc(strlen(init_dir) + 10);
      sprintf(path, "%s\\init.bat", init_dir);
      argv[n++] = path;
    }
  }

  argv[n] = NULL;

  return argv;
}

static char** build_env(struct pss_tty* pss) {
  int i = 0, n = 3;
  char** envp = xmalloc(n * sizeof(char*));

  envp[i] = xmalloc(36);
  snprintf(envp[i], 36, "TERM=%s", server->terminal_type);
  i++;

  envp[i] = xmalloc(24);
  snprintf(envp[i], 24, "COLORTERM=truecolor");
  i++;

#ifndef _WIN32
  if (pss->notify != NULL) {
    const char* shim = notify_shim_dir(pss->notify);
    const char* init = notify_init_dir(pss->notify);

    envp = xrealloc(envp, (++n) * sizeof(char*));
    envp[i] = xmalloc(256);
    snprintf(envp[i], 256, "TABSH_SHIM_DIR=%s", shim);
    i++;

    envp = xrealloc(envp, (++n) * sizeof(char*));
    envp[i] = xmalloc(256);
    snprintf(envp[i], 256, "TABSH_INIT_DIR=%s", init);
    i++;

    const char* old_path = getenv("PATH");
    if (old_path != NULL) {
      size_t path_len = strlen(shim) + 1 + strlen(old_path) + 6;
      envp = xrealloc(envp, (++n) * sizeof(char*));
      envp[i] = xmalloc(path_len);
      snprintf(envp[i], path_len, "PATH=%s:%s", shim, old_path);
      i++;
    }

    const char* shell = detect_shell(server->argv[0]);
    if (shell != NULL) {
      if (strcmp(shell, "zsh") == 0) {
        const char* orig_zdotdir = getenv("ZDOTDIR");
        if (orig_zdotdir != NULL) {
          envp = xrealloc(envp, (++n) * sizeof(char*));
          envp[i] = xmalloc(256);
          snprintf(envp[i], 256, "TABSH_ORIG_ZDOTDIR=%s", orig_zdotdir);
          i++;
        }
        envp = xrealloc(envp, (++n) * sizeof(char*));
        envp[i] = xmalloc(256);
        snprintf(envp[i], 256, "ZDOTDIR=%s/zsh", init);
        i++;
      } else if (strcmp(shell, "fish") == 0) {
        envp = xrealloc(envp, (++n) * sizeof(char*));
        envp[i] = xmalloc(256);
        snprintf(envp[i], 256, "XDG_CONFIG_HOME=%s/fish", init);
        i++;
      } else if (strcmp(shell, "sh") == 0 || strcmp(shell, "dash") == 0 || strcmp(shell, "ksh") == 0) {
        const char* orig_env = getenv("ENV");
        if (orig_env != NULL) {
          envp = xrealloc(envp, (++n) * sizeof(char*));
          envp[i] = xmalloc(256);
          snprintf(envp[i], 256, "TABSH_ORIG_ENV=%s", orig_env);
          i++;
        }
        envp = xrealloc(envp, (++n) * sizeof(char*));
        envp[i] = xmalloc(256);
        snprintf(envp[i], 256, "ENV=%s/env.sh", init);
        i++;
      }
    }
  }
#endif

  envp[i] = NULL;

  return envp;
}

static bool spawn_process(struct pss_tty* pss, const char* session_id, uint16_t columns, uint16_t rows) {
#ifndef _WIN32
  notify_ctx_init(&pss->notify, session_id);
#endif
  pty_process* process = process_init(NULL, server->loop, build_args(pss), build_env(pss));
  session_t* session = session_create(session_id, process, pss);
  process->ctx = (void*)session;
#ifndef _WIN32
  session->notify = pss->notify;
#endif
  const char* cwd = pss->cwd ? pss->cwd : server->cwd;
  if (cwd != NULL) process->cwd = strdup(cwd);
  if (columns > 0) process->columns = columns;
  if (rows > 0) process->rows = rows;
  if (pty_spawn(process, process_read_cb, process_exit_cb) != 0) {
    lwsl_err("pty_spawn: %d (%s)\n", errno, strerror(errno));
    uv_close((uv_handle_t*)session->timer, (uv_close_cb)free);
    process_free(process);
#ifndef _WIN32
    if (pss->notify != NULL) {
      notify_ctx_destroy(pss->notify);
      pss->notify = NULL;
    }
#endif
    free(session);
    return false;
  }
  lwsl_notice("started process, pid: %d\n", process->pid);
  session->root_pid = process->pid;
  session->terminal =
      terminal_create(process->rows ? process->rows : 24, process->columns ? process->columns : 80, pss);
  pss->process = process;
  pss->session = session;
  strncpy(pss->session_id, session_id, SESSION_ID_LEN - 1);
  pss->session_id[SESSION_ID_LEN - 1] = '\0';
  if (pss->app_command != NULL) {
    size_t app_len = strlen(pss->app_command);
    char* input = xmalloc(app_len + 2);
    memcpy(input, pss->app_command, app_len);
    input[app_len] = '\r';
    input[app_len + 1] = '\0';
    int err = pty_write(process, pty_buf_init(input, app_len + 1));
    free(input);
    if (err) lwsl_err("write app command: %s (%s)\n", uv_err_name(err), uv_strerror(err));
  }
  lws_callback_on_writable(pss->wsi);

  return true;
}


static void attach_session(struct pss_tty* pss, session_t* session) {
  session_attach(session, pss);
  pss->process = session->process;
  pss->session = session;
  if (session->terminal != NULL) {
    terminal_replay_sb(session->terminal);
    terminal_mark_all_dirty(session->terminal);
    pss->pending_frame = true;
  }
  lws_callback_on_writable(pss->wsi);
}

int callback_tty(struct lws* wsi, enum lws_callback_reasons reason, void* user, void* in, size_t len) {
  struct pss_tty* pss = (struct pss_tty*)user;
  char buf[256];
  size_t n = 0;

  switch (reason) {
    case LWS_CALLBACK_FILTER_PROTOCOL_CONNECTION:
      if (server->max_clients > 0 && server->client_count == server->max_clients) {
        lwsl_warn("refuse to serve WS client due to the --max-clients option.\n");
        return 1;
      }
      n = lws_hdr_copy(wsi, pss->path, sizeof(pss->path), WSI_TOKEN_GET_URI);
#if defined(LWS_ROLE_H2)
      if (n <= 0) n = lws_hdr_copy(wsi, pss->path, sizeof(pss->path), WSI_TOKEN_HTTP_COLON_PATH);
#endif
      if (strncmp(pss->path, endpoints.ws, n) != 0) {
        lwsl_warn("refuse to serve WS client for illegal ws path: %s\n", pss->path);
        return 1;
      }
      break;

    case LWS_CALLBACK_ESTABLISHED:
      pss->initialized = false;
      pss->intentional_close = false;
      pss->wsi = wsi;
      pss->lws_close_status = LWS_CLOSE_STATUS_NOSTATUS;
      pss->session = NULL;
      pss->session_id[0] = '\0';
      pss->notify = NULL;
      server->client_count++;
      favicon_pss_add(pss);

      lws_get_peer_simple(lws_get_network_wsi(wsi), pss->address, sizeof(pss->address));
      lwsl_notice("WS   %s - %s, clients: %d\n", pss->path, pss->address, server->client_count);
      break;

    case LWS_CALLBACK_SERVER_WRITEABLE:
      /* libwebsockets requires exactly one lws_write per LWS_CALLBACK_SERVER_WRITEABLE
       * invocation. Every path below must write at most one message then break.
       * Use lws_callback_on_writable to schedule subsequent sends. */
      if (!pss->initialized) {
        if (pss->initial_cmd_index == sizeof(initial_cmds)) {
          pss->initialized = true;
          pty_resume(pss->process);
          if (pss->reattached && pss->process != NULL) {
            pss->reattached = false;
            /* Queue state re-sends for subsequent callbacks, then send REATTACHED now. */
            if (pss->session && pss->session->terminal) {
              pss->pending_mouse_mode_send = true;
              pss->pending_mouse_mode_value = pss->session->terminal->mouse_mode;
              pss->pending_altscreen_send = true;
              pss->pending_altscreen_value = pss->session->terminal->altscreen_active;
              pss->pending_cursor_blink_send = true;
              pss->pending_cursor_blink_value = pss->session->terminal->cursor_blink_enabled;
            }
            uv_timer_t* t = xmalloc(sizeof(uv_timer_t));
            uv_timer_init(server->loop, t);
            t->data = pss->process;
            uv_timer_start(t, reattach_sigwinch_cb, 100, 0);
            unsigned char reattach_msg[LWS_PRE + 1];
            reattach_msg[LWS_PRE] = REATTACHED;
            lws_write(wsi, &reattach_msg[LWS_PRE], 1, LWS_WRITE_BINARY);
            lws_callback_on_writable(wsi);
          }
          break;
        }
        if (send_initial_message(wsi, pss->initial_cmd_index) < 0) {
          lwsl_err("failed to send initial message, index: %d\n", pss->initial_cmd_index);
          lws_close_reason(wsi, LWS_CLOSE_STATUS_UNEXPECTED_CONDITION, NULL, 0);
          return -1;
        }
        pss->initial_cmd_index++;
        lws_callback_on_writable(wsi);
        break;
      }

      if (pss->lws_close_status > LWS_CLOSE_STATUS_NOSTATUS) {
        lws_close_reason(wsi, pss->lws_close_status, NULL, 0);
        return 1;
      }

      if (pss->pending_altscreen_send) {
        pss->pending_altscreen_send = false;
        unsigned char as[LWS_PRE + 2];
        as[LWS_PRE] = ALT_SCREEN;
        as[LWS_PRE + 1] = pss->pending_altscreen_value;
        lws_write(wsi, &as[LWS_PRE], 2, LWS_WRITE_BINARY);
        lws_callback_on_writable(pss->wsi);
        break;
      }

#ifndef _WIN32
      if (pss->pending_app_send) {
        pss->pending_app_send = false;
        size_t app_len = strlen(pss->pending_app);
        unsigned char* msg = xmalloc(LWS_PRE + 1 + app_len);
        unsigned char* p = msg + LWS_PRE;
        p[0] = APP_COMMAND;
        memcpy(p + 1, pss->pending_app, app_len);
        lws_write(wsi, p, 1 + app_len, LWS_WRITE_BINARY);
        free(msg);
        lws_callback_on_writable(wsi);
        break;
      }

      if (pss->pending_favicon_send) {
        pss->pending_favicon_send = false;
        size_t flen = strlen(pss->pending_favicon);
        unsigned char* msg = xmalloc(LWS_PRE + 1 + flen);
        unsigned char* p = msg + LWS_PRE;
        p[0] = APP_FAVICON;
        memcpy(p + 1, pss->pending_favicon, flen);
        lws_write(wsi, p, 1 + flen, LWS_WRITE_BINARY);
        free(msg);
        lws_callback_on_writable(wsi);
        break;
      }
#endif

      if (pss->session != NULL && pss->session->terminal != NULL) {
        /* Drain one scrollback line per callback — history before refresh. */
        size_t sblen;
        const unsigned char* sb = terminal_take_sb_line(pss->session->terminal, &sblen);
        if (sb != NULL) {
          if (lws_write(wsi, (unsigned char*)sb, sblen, LWS_WRITE_BINARY) < (int)sblen)
            lwsl_err("write SB_PUSH\n");
          lws_callback_on_writable(wsi);
          break;
        }
      }

      if (pss->pending_frame && pss->session != NULL && pss->session->terminal != NULL) {
        pss->pending_frame = false;
        size_t frame_len;
        const unsigned char* frame = terminal_encode_frame(pss->session->terminal, &frame_len);
        if (lws_write(wsi, (unsigned char*)frame, frame_len, LWS_WRITE_BINARY) < (int)frame_len)
          lwsl_err("write CELL_DIFF\n");
        lws_callback_on_writable(wsi);
        break;
      }

      if (pss->session != NULL && pss->session->terminal != NULL) {
        /* Collect any new terminal state changes into pending fields. */
        if (!pss->pending_mouse_mode_send) {
          uint8_t mode = 0;
          if (terminal_take_mouse_mode_change(pss->session->terminal, &mode)) {
            pss->pending_mouse_mode_send = true;
            pss->pending_mouse_mode_value = mode;
          }
        }
        if (!pss->pending_cursor_blink_send) {
          bool blink = false;
          if (terminal_take_cursor_blink_change(pss->session->terminal, &blink)) {
            pss->pending_cursor_blink_send = true;
            pss->pending_cursor_blink_value = blink;
          }
        }
      }

      if (pss->pending_mouse_mode_send) {
        pss->pending_mouse_mode_send = false;
        unsigned char buf2[LWS_PRE + 2];
        buf2[LWS_PRE] = MOUSE_MODE;
        buf2[LWS_PRE + 1] = pss->pending_mouse_mode_value;
        lws_write(wsi, &buf2[LWS_PRE], 2, LWS_WRITE_BINARY);
        lws_callback_on_writable(wsi);
        break;
      }

      if (pss->pending_cursor_blink_send) {
        pss->pending_cursor_blink_send = false;
        unsigned char buf3[LWS_PRE + 2];
        buf3[LWS_PRE] = CURSOR_BLINK;
        buf3[LWS_PRE + 1] = pss->pending_cursor_blink_value ? 1 : 0;
        lws_write(wsi, &buf3[LWS_PRE], 2, LWS_WRITE_BINARY);
        lws_callback_on_writable(wsi);
        break;
      }

      if (pss->session != NULL && pss->session->terminal != NULL) {
        char *title = terminal_take_title(pss->session->terminal);
        if (title) {
          size_t tlen = strlen(title);
          unsigned char *tmsg = xmalloc(LWS_PRE + 1 + tlen);
          tmsg[LWS_PRE] = WINDOW_TITLE;
          memcpy(tmsg + LWS_PRE + 1, title, tlen);
          lws_write(wsi, tmsg + LWS_PRE, 1 + tlen, LWS_WRITE_BINARY);
          free(tmsg);
          free(title);
          lws_callback_on_writable(wsi);
          break;
        }
        uint8_t altv = 0;
        if (terminal_take_altscreen_change(pss->session->terminal, &altv)) {
          pss->pending_altscreen_send = true;
          pss->pending_altscreen_value = altv;
          pss->pending_frame = true;
          lws_callback_on_writable(pss->wsi);
        }
      }
      break;

    case LWS_CALLBACK_RECEIVE:
      if (pss->buffer == NULL) {
        pss->buffer = xmalloc(len);
        pss->len = len;
        memcpy(pss->buffer, in, len);
      } else {
        pss->buffer = xrealloc(pss->buffer, pss->len + len);
        memcpy(pss->buffer + pss->len, in, len);
        pss->len += len;
      }

      const char command = pss->buffer[0];

      // check if there are more fragmented messages
      if (lws_remaining_packet_payload(wsi) > 0 || !lws_is_final_fragment(wsi)) {
        return 0;
      }

      switch (command) {
        case INPUT: {
          int err = pty_write(pss->process, pty_buf_init(pss->buffer + 1, pss->len - 1));
          if (err) {
            lwsl_err("uv_write: %s (%s)\n", uv_err_name(err), uv_strerror(err));
            return -1;
          }
          break;
        }
        case CLEAR:
          if (pss->session != NULL && pss->session->terminal != NULL)
            terminal_clear(pss->session->terminal);
          break;
        case RESIZE:
          if (pss->process == NULL) break;
          json_object_put(
              parse_window_size(pss->buffer + 1, pss->len - 1, &pss->process->columns, &pss->process->rows));
          pty_resize(pss->process);
          if (pss->session != NULL && pss->session->terminal != NULL) {
            terminal_resize(pss->session->terminal, pss->process->rows, pss->process->columns);
            terminal_mark_all_dirty(pss->session->terminal);
            pss->pending_frame = true;
            lws_callback_on_writable(pss->wsi);
          }
          break;
        case PAUSE:
          pty_pause(pss->process);
          break;
        case RESUME:
          pty_resume(pss->process);
          break;
        case QUIT:
          pss->intentional_close = true;
          break;
        case JSON_DATA:
          if (pss->process != NULL) break;
          uint16_t columns = 0;
          uint16_t rows = 0;
          json_object* obj = parse_window_size(pss->buffer, pss->len, &columns, &rows);

          struct json_object* sid = NULL;
          const char* client_session_id = NULL;
          if (json_object_object_get_ex(obj, "sessionId", &sid)) client_session_id = json_object_get_string(sid);

          if (client_session_id == NULL || strlen(client_session_id) != 36) {
            lwsl_err("missing or invalid sessionId in client message\n");
            json_object_put(obj);
            return -1;
          }

          session_t* existing = session_find(client_session_id);
          if (existing != NULL && existing->detached && process_running(existing->process)) {
            lwsl_notice("session %s reconnected\n", client_session_id);
            json_object_put(obj);
            attach_session(pss, existing);
            break;
          }

          struct json_object* cwd_obj = NULL;
          if (json_object_object_get_ex(obj, "cwd", &cwd_obj)) {
            const char* cwd_str = json_object_get_string(cwd_obj);
            if (cwd_str != NULL && strlen(cwd_str) > 0) pss->cwd = strdup(cwd_str);
          }

          struct json_object* app_obj = NULL;
          if (json_object_object_get_ex(obj, "app", &app_obj)) {
            const char* app_str = json_object_get_string(app_obj);
            if (app_str != NULL && strlen(app_str) > 0) pss->app_command = strdup(app_str);
          }

          if (!spawn_process(pss, client_session_id, columns, rows)) {
            json_object_put(obj);
            return 1;
          }

          json_object_put(obj);

          break;
        default:
          lwsl_warn("ignored unknown message type: %c\n", command);
          break;
      }

      if (pss->buffer != NULL) {
        free(pss->buffer);
        pss->buffer = NULL;
      }
      break;

    case LWS_CALLBACK_CLOSED:
      if (pss->wsi == NULL) break;

      favicon_pss_remove(pss);
      server->client_count--;
      lwsl_notice("WS closed from %s, clients: %d\n", pss->address, server->client_count);
      if (pss->buffer != NULL) free(pss->buffer);
      if (pss->cwd != NULL) {
        free(pss->cwd);
        pss->cwd = NULL;
      }
      if (pss->app_command != NULL) {
        free(pss->app_command);
        pss->app_command = NULL;
      }
      if (pss->process != NULL && process_running(pss->process)) {
        if (pss->session != NULL) {
#ifndef _WIN32
          if (pss->notify != NULL) {
            if (pss->intentional_close) {
              notify_ctx_destroy(pss->notify);
              pss->session->notify = NULL;
            } else {
              pss->session->notify = pss->notify;
            }
            pss->notify = NULL;
          }
#endif
          if (pss->intentional_close) {
            session_stop(pss->session);
          } else {
            lwsl_notice("detaching session %s for client %s\n", pss->session_id, pss->address);
            session_detach(pss->session);
          }
          pss->session = NULL;
        } else {
#ifndef _WIN32
          if (pss->notify != NULL) {
            notify_ctx_destroy(pss->notify);
            pss->notify = NULL;
          }
#endif
          pty_pause(pss->process);
          lwsl_notice("killing process, pid: %d\n", pss->process->pid);
          pty_kill(pss->process, server->sig_code);
        }
      }

      break;

    default:
      break;
  }

  return 0;
}
