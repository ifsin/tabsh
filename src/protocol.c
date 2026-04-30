#include <errno.h>
#include <json.h>
#include <libwebsockets.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifndef _WIN32
#include <fcntl.h>
#include <signal.h>
#include <termios.h>
#include <unistd.h>
#ifdef __APPLE__
#include <sys/sysctl.h>
#endif
#else
#include <psapi.h>
#include <tlhelp32.h>
#include <windows.h>
#endif

#include "pty.h"
#include "server.h"
#include "utils.h"
#include "compat.h"

// initial message list
static char initial_cmds[] = {SET_WINDOW_TITLE, SET_PREFERENCES};

#ifndef _WIN32
static void get_process_argv(pid_t pid, char *out, size_t out_len) {
  out[0] = '\0';
#ifdef __APPLE__
  int mib[3] = {CTL_KERN, KERN_PROCARGS2, pid};
  char buf[8192];
  size_t buf_size = sizeof(buf);
  if (sysctl(mib, 3, buf, &buf_size, NULL, 0) != 0) return;
  int argc = *(int *)buf;
  char *p = buf + sizeof(int);
  char *end = buf + buf_size;
  p += strnlen(p, (size_t)(end - p)) + 1;
  while (p < end && *p == '\0') p++;
  size_t written = 0;
  for (int i = 0; i < argc && p < end && written < out_len - 1; i++) {
    if (i > 0 && written < out_len - 1) out[written++] = ' ';
    size_t arg_len = strnlen(p, out_len - written - 1);
    memcpy(out + written, p, arg_len);
    written += arg_len;
    p += strnlen(p, (size_t)(end - p)) + 1;
  }
  out[written] = '\0';
#else
  char path[64];
  snprintf(path, sizeof(path), "/proc/%d/cmdline", (int)pid);
  int fd = open(path, O_RDONLY);
  if (fd < 0) return;
  char buf[8192];
  ssize_t n = read(fd, buf, sizeof(buf) - 1);
  close(fd);
  if (n <= 0) return;
  size_t written = 0;
  for (ssize_t i = 0; i < n && written < out_len - 1; i++) {
    out[written++] = buf[i] == '\0' ? ' ' : buf[i];
  }
  while (written > 0 && out[written - 1] == ' ') written--;
  out[written] = '\0';
#endif
}

static void app_detect_cb(uv_timer_t *timer) {
  struct pss_tty *pss = (struct pss_tty *)timer->data;
  if (pss == NULL || pss->process == NULL || pss->wsi == NULL) return;

  struct termios tios;
  if (tcgetattr(pss->process->pty, &tios) != 0) return;

  bool is_raw = !(tios.c_lflag & ICANON) && !(tios.c_lflag & ECHO);
  char app[512] = {0};

  if (is_raw) {
    pid_t fgpid = tcgetpgrp(pss->process->pty);
    if (fgpid > 0) get_process_argv(fgpid, app, sizeof(app));
  }

  if (strcmp(app, pss->current_app) != 0) {
    strncpy(pss->current_app, app, sizeof(pss->current_app) - 1);
    pss->current_app[sizeof(pss->current_app) - 1] = '\0';
    strncpy(pss->pending_app, app, sizeof(pss->pending_app) - 1);
    pss->pending_app[sizeof(pss->pending_app) - 1] = '\0';
    pss->pending_app_send = true;
    lws_callback_on_writable(pss->wsi);
  }
}
#else
static bool is_shell(const char *name) {
  const char *shells[] = {"cmd.exe", "powershell.exe", "pwsh.exe", "bash.exe", "zsh.exe", "sh.exe", NULL};
  for (int i = 0; shells[i] != NULL; i++) {
    if (_stricmp(name, shells[i]) == 0) return true;
  }
  return false;
}

static void get_child_app(DWORD parent_pid, char *out, size_t out_len) {
  out[0] = '\0';
  HANDLE hSnap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
  if (hSnap == INVALID_HANDLE_VALUE) return;

  PROCESSENTRY32 pe;
  pe.dwSize = sizeof(pe);
  DWORD child_pid = 0;

  if (Process32First(hSnap, &pe)) {
    do {
      if (pe.th32ParentProcessID == parent_pid && !is_shell(pe.szExeFile)) {
        child_pid = pe.th32ProcessID;
        break;
      }
    } while (Process32Next(hSnap, &pe));
  }
  CloseHandle(hSnap);

  if (child_pid == 0) return;

  HANDLE hProc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, child_pid);
  if (hProc == NULL) return;

  char path[MAX_PATH];
  DWORD path_len = MAX_PATH;
  if (QueryFullProcessImageNameA(hProc, 0, path, &path_len)) {
    char *name = strrchr(path, '\\');
    strncpy(out, name ? name + 1 : path, out_len - 1);
    out[out_len - 1] = '\0';
    char *ext = strrchr(out, '.');
    if (ext && _stricmp(ext, ".exe") == 0) *ext = '\0';
  }
  CloseHandle(hProc);
}

static void app_detect_cb(uv_timer_t *timer) {
  struct pss_tty *pss = (struct pss_tty *)timer->data;
  if (pss == NULL || pss->process == NULL || pss->wsi == NULL) return;

  char app[512] = {0};
  get_child_app((DWORD)pss->process->pid, app, sizeof(app));

  if (strcmp(app, pss->current_app) != 0) {
    strncpy(pss->current_app, app, sizeof(pss->current_app) - 1);
    pss->current_app[sizeof(pss->current_app) - 1] = '\0';
    strncpy(pss->pending_app, app, sizeof(pss->pending_app) - 1);
    pss->pending_app[sizeof(pss->pending_app) - 1] = '\0';
    pss->pending_app_send = true;
    lws_callback_on_writable(pss->wsi);
  }
}
#endif

static void reattach_sigwinch_cb(uv_timer_t *timer) {
  pty_process *process = (pty_process *)timer->data;
#ifndef _WIN32
  if (process != NULL && process->pid > 0) {
    pty_resize(process);
    uv_kill(-process->pid, SIGWINCH);
  }
#endif
  uv_close((uv_handle_t *)timer, (uv_close_cb)free);
}

static int send_initial_message(struct lws *wsi, int index) {
  unsigned char message[LWS_PRE + 1 + 4096];
  unsigned char *p = &message[LWS_PRE];
  int n = 0;

  char cmd = initial_cmds[index];
  switch (cmd) {
    case SET_WINDOW_TITLE:
      n = snprintf((char *)p, 1 + 4096, "%c", cmd);
      break;
    case SET_PREFERENCES:
      n = snprintf((char *)p, 1 + 4096, "%c%s", cmd, server->prefs_json);
      break;
    default:
      break;
  }

  return lws_write(wsi, p, (size_t)n, LWS_WRITE_BINARY);
}

static json_object *parse_window_size(const char *buf, size_t len, uint16_t *cols, uint16_t *rows) {
  json_tokener *tok = json_tokener_new();
  json_object *obj = json_tokener_parse_ex(tok, buf, len);
  struct json_object *o = NULL;

  if (json_object_object_get_ex(obj, "columns", &o)) *cols = (uint16_t)json_object_get_int(o);
  if (json_object_object_get_ex(obj, "rows", &o)) *rows = (uint16_t)json_object_get_int(o);

  json_tokener_free(tok);
  return obj;
}

static bool check_host_origin(struct lws *wsi) {
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

static void process_read_cb(pty_process *process, pty_buf_t *buf, bool eof) {
  session_t *session = (session_t *)process->ctx;
  if (session->detached) {
    pty_buf_free(buf);
    return;
  }

  if (eof && !process_running(process))
    session->pss->lws_close_status = process->exit_code == 0 ? 1000 : 1006;
  else
    session->pss->pty_buf = buf;
  lws_callback_on_writable(session->pss->wsi);
}

static void process_exit_cb(pty_process *process) {
  session_t *session = (session_t *)process->ctx;
  struct pss_tty *pss = session->pss;
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
  session_remove(session);
  if (session->timer != NULL) {
    uv_timer_stop(session->timer);
    uv_close((uv_handle_t *)session->timer, (uv_close_cb)free);
  }
  free(session);

  // if we are going to exit, do it now.
  if (force_exit) exit(0);
}

static char **build_args(struct pss_tty *pss) {
  int i, n = 0;
  char **argv = xmalloc((server->argc + pss->argc + 1) * sizeof(char *));

  for (i = 0; i < server->argc; i++) {
    argv[n++] = server->argv[i];
  }

  for (i = 0; i < pss->argc; i++) {
    argv[n++] = pss->args[i];
  }

  argv[n] = NULL;

  return argv;
}

static char **build_env(struct pss_tty *pss) {
  int i = 0, n = 2;
  char **envp = xmalloc(n * sizeof(char *));

  // TERM
  envp[i] = xmalloc(36);
  snprintf(envp[i], 36, "TERM=%s", server->terminal_type);
  i++;

  // TTYD_USER
  if (strlen(pss->user) > 0) {
    envp = xrealloc(envp, (++n) * sizeof(char *));
    envp[i] = xmalloc(40);
    snprintf(envp[i], 40, "TTYD_USER=%s", pss->user);
    i++;
  }

  envp[i] = NULL;

  return envp;
}

static bool spawn_process(struct pss_tty *pss, const char *session_id, uint16_t columns, uint16_t rows) {
  pty_process *process = process_init(NULL, server->loop, build_args(pss), build_env(pss));
  session_t *session = session_create(session_id, process, pss);
  process->ctx = (void *)session;
  if (server->cwd != NULL) process->cwd = strdup(server->cwd);
  if (columns > 0) process->columns = columns;
  if (rows > 0) process->rows = rows;
  if (pty_spawn(process, process_read_cb, process_exit_cb) != 0) {
    lwsl_err("pty_spawn: %d (%s)\n", errno, strerror(errno));
    uv_close((uv_handle_t *)session->timer, (uv_close_cb)free);
    process_free(process);
    free(session);
    return false;
  }
  lwsl_notice("started process, pid: %d\n", process->pid);
  pss->process = process;
  pss->session = session;
  strncpy(pss->session_id, session_id, SESSION_ID_LEN - 1);
  pss->session_id[SESSION_ID_LEN - 1] = '\0';
  lws_callback_on_writable(pss->wsi);

  return true;
}

static void wsi_output(struct lws *wsi, pty_buf_t *buf) {
  if (buf == NULL) return;
  char *message = xmalloc(LWS_PRE + 1 + buf->len);
  char *ptr = message + LWS_PRE;

  *ptr = OUTPUT;
  memcpy(ptr + 1, buf->base, buf->len);
  size_t n = buf->len + 1;

  if (lws_write(wsi, (unsigned char *)ptr, n, LWS_WRITE_BINARY) < n) {
    lwsl_err("write OUTPUT to WS\n");
  }

  free(message);
}

static bool check_auth(struct lws *wsi, struct pss_tty *pss) {
  if (server->auth_header != NULL) {
    return lws_hdr_custom_copy(wsi, pss->user, sizeof(pss->user), server->auth_header, strlen(server->auth_header)) > 0;
  }

  if (server->credential != NULL) {
    char buf[256];
    size_t n = lws_hdr_copy(wsi, buf, sizeof(buf), WSI_TOKEN_HTTP_AUTHORIZATION);
    return n >= 7 && strstr(buf, "Basic ") && !strcmp(buf + 6, server->credential);
  }

  return true;
}

static void attach_session(struct pss_tty *pss, session_t *session) {
  session_attach(session, pss);
  pss->process = session->process;
  pss->session = session;
  lws_callback_on_writable(pss->wsi);
}

int callback_tty(struct lws *wsi, enum lws_callback_reasons reason, void *user, void *in, size_t len) {
  struct pss_tty *pss = (struct pss_tty *)user;
  char buf[256];
  size_t n = 0;

  switch (reason) {
    case LWS_CALLBACK_FILTER_PROTOCOL_CONNECTION:
      if (server->once && server->client_count > 0) {
        lwsl_warn("refuse to serve WS client due to the --once option.\n");
        return 1;
      }
      if (server->max_clients > 0 && server->client_count == server->max_clients) {
        lwsl_warn("refuse to serve WS client due to the --max-clients option.\n");
        return 1;
      }
      if (!check_auth(wsi, pss)) return 1;

      n = lws_hdr_copy(wsi, pss->path, sizeof(pss->path), WSI_TOKEN_GET_URI);
#if defined(LWS_ROLE_H2)
      if (n <= 0) n = lws_hdr_copy(wsi, pss->path, sizeof(pss->path), WSI_TOKEN_HTTP_COLON_PATH);
#endif
      if (strncmp(pss->path, endpoints.ws, n) != 0) {
        lwsl_warn("refuse to serve WS client for illegal ws path: %s\n", pss->path);
        return 1;
      }

      if (server->check_origin && !check_host_origin(wsi)) {
        lwsl_warn(
            "refuse to serve WS client from different origin due to the "
            "--check-origin option.\n");
        return 1;
      }
      break;

    case LWS_CALLBACK_ESTABLISHED:
      pss->initialized = false;
      pss->authenticated = false;
      pss->intentional_close = false;
      pss->wsi = wsi;
      pss->lws_close_status = LWS_CLOSE_STATUS_NOSTATUS;
      pss->session = NULL;
      pss->session_id[0] = '\0';

      if (server->url_arg) {
        while (lws_hdr_copy_fragment(wsi, buf, sizeof(buf), WSI_TOKEN_HTTP_URI_ARGS, n++) > 0) {
          if (strncmp(buf, "arg=", 4) == 0) {
            pss->args = xrealloc(pss->args, (pss->argc + 1) * sizeof(char *));
            pss->args[pss->argc] = strdup(&buf[4]);
            pss->argc++;
          }
        }
      }

      server->client_count++;

      lws_get_peer_simple(lws_get_network_wsi(wsi), pss->address, sizeof(pss->address));
      lwsl_notice("WS   %s - %s, clients: %d\n", pss->path, pss->address, server->client_count);
      break;

    case LWS_CALLBACK_SERVER_WRITEABLE:
      if (!pss->initialized) {
        if (pss->initial_cmd_index == sizeof(initial_cmds)) {
          pss->initialized = true;
          pty_resume(pss->process);
          if (pss->reattached && pss->process != NULL) {
            pss->reattached = false;
            unsigned char reattach_msg[LWS_PRE + 1];
            reattach_msg[LWS_PRE] = SET_REATTACHED;
            lws_write(wsi, &reattach_msg[LWS_PRE], 1, LWS_WRITE_BINARY);
            uv_timer_t *t = xmalloc(sizeof(uv_timer_t));
            uv_timer_init(server->loop, t);
            t->data = pss->process;
            uv_timer_start(t, reattach_sigwinch_cb, 100, 0);
          }
          pss->app_timer = xmalloc(sizeof(uv_timer_t));
          uv_timer_init(server->loop, pss->app_timer);
          pss->app_timer->data = pss;
          uv_timer_start(pss->app_timer, app_detect_cb, 200, 200);
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

      if (pss->pending_app_send) {
        pss->pending_app_send = false;
        size_t app_len = strlen(pss->pending_app);
        unsigned char *msg = xmalloc(LWS_PRE + 1 + app_len);
        unsigned char *p = msg + LWS_PRE;
        p[0] = SET_APP_COMMAND;
        memcpy(p + 1, pss->pending_app, app_len);
        lws_write(wsi, p, 1 + app_len, LWS_WRITE_BINARY);
        free(msg);
      }

      if (pss->pty_buf != NULL) {
        wsi_output(wsi, pss->pty_buf);
        pty_buf_free(pss->pty_buf);
        pss->pty_buf = NULL;
        pty_resume(pss->process);
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

      // check auth
      if (server->credential != NULL && !pss->authenticated && command != JSON_DATA) {
        lwsl_warn("WS client not authenticated\n");
        return 1;
      }

      // check if there are more fragmented messages
      if (lws_remaining_packet_payload(wsi) > 0 || !lws_is_final_fragment(wsi)) {
        return 0;
      }

      switch (command) {
        case INPUT:
          if (!server->writable) break;
          int err = pty_write(pss->process, pty_buf_init(pss->buffer + 1, pss->len - 1));
          if (err) {
            lwsl_err("uv_write: %s (%s)\n", uv_err_name(err), uv_strerror(err));
            return -1;
          }
          break;
        case RESIZE_TERMINAL:
          if (pss->process == NULL) break;
          json_object_put(
              parse_window_size(pss->buffer + 1, pss->len - 1, &pss->process->columns, &pss->process->rows));
          pty_resize(pss->process);
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
          json_object *obj = parse_window_size(pss->buffer, pss->len, &columns, &rows);
          if (server->credential != NULL) {
            struct json_object *o = NULL;
            if (json_object_object_get_ex(obj, "AuthToken", &o)) {
              const char *token = json_object_get_string(o);
              if (token != NULL && !strcmp(token, server->credential))
                pss->authenticated = true;
              else
                lwsl_warn("WS authentication failed with token: %s\n", token);
            }
            if (!pss->authenticated) {
              json_object_put(obj);
              lws_close_reason(wsi, LWS_CLOSE_STATUS_POLICY_VIOLATION, NULL, 0);
              return -1;
            }
          }

          struct json_object *sid = NULL;
          const char *client_session_id = NULL;
          if (json_object_object_get_ex(obj, "sessionId", &sid))
            client_session_id = json_object_get_string(sid);

          if (client_session_id == NULL || strlen(client_session_id) != 36) {
            lwsl_err("missing or invalid sessionId in client message\n");
            json_object_put(obj);
            return -1;
          }

          session_t *existing = session_find(client_session_id);
          if (existing != NULL && existing->detached && process_running(existing->process)) {
            lwsl_notice("session %s reconnected\n", client_session_id);
            json_object_put(obj);
            attach_session(pss, existing);
            break;
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

      server->client_count--;
      lwsl_notice("WS closed from %s, clients: %d\n", pss->address, server->client_count);
      if (pss->buffer != NULL) free(pss->buffer);
      if (pss->pty_buf != NULL) pty_buf_free(pss->pty_buf);
      if (pss->app_timer != NULL) {
        uv_timer_stop(pss->app_timer);
        uv_close((uv_handle_t *)pss->app_timer, (uv_close_cb)free);
        pss->app_timer = NULL;
      }
      for (int i = 0; i < pss->argc; i++) {
        free(pss->args[i]);
      }

      if (pss->process != NULL && process_running(pss->process)) {
        if (pss->session != NULL) {
          if (pss->intentional_close) {
            session_stop(pss->session);
          } else {
            lwsl_notice("detaching session %s for client %s\n", pss->session_id, pss->address);
            session_detach(pss->session);
          }
          pss->session = NULL;
        } else {
          pty_pause(pss->process);
          lwsl_notice("killing process, pid: %d\n", pss->process->pid);
          pty_kill(pss->process, server->sig_code);
        }
      }

      if ((server->once || server->exit_no_conn) && server->client_count == 0) {
        lwsl_notice("exiting due to the --once/--exit-no-conn option.\n");

        // stop accepting new ws connections
        lws_cancel_service(context);

        if (process_running(pss->process)) {
          force_exit = true;
          lwsl_notice("send ^C to force exit.\n");
        } else {
          exit(0);
        }
      }
      break;

    default:
      break;
  }

  return 0;
}
