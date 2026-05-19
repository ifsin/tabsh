#include "server.h"

#include <errno.h>
#include <getopt.h>
#include <json.h>
#include <libwebsockets.h>
#include <signal.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

#include "utils.h"
#include "compat.h"

#ifndef TTYD_VERSION
#define TTYD_VERSION "unknown"
#endif

volatile bool force_exit = false;
struct lws_context *context;
struct server *server;
struct endpoints endpoints = {"/ws", "/", "/Bell.mp3", "/favicon/", "/content"};

extern int callback_http(struct lws *wsi, enum lws_callback_reasons reason, void *user, void *in, size_t len);
extern int callback_tty(struct lws *wsi, enum lws_callback_reasons reason, void *user, void *in, size_t len);

// websocket protocols
static const struct lws_protocols protocols[] = {{"http-only", callback_http, sizeof(struct pss_http), 0},
                                                 {"tty", callback_tty, sizeof(struct pss_tty), 0},
                                                 {NULL, NULL, 0, 0}};

#ifndef LWS_WITHOUT_EXTENSIONS
// websocket extensions
static const struct lws_extension extensions[] = {
    {"permessage-deflate", lws_extension_callback_pm_deflate, "permessage-deflate"},
    {"deflate-frame", lws_extension_callback_pm_deflate, "deflate_frame"},
    {NULL, NULL, NULL}};
#endif

#if LWS_LIBRARY_VERSION_NUMBER >= 4000000
static const uint32_t backoff_ms[] = {1000, 2000, 3000, 4000, 5000};
static lws_retry_bo_t retry = {
    .retry_ms_table = backoff_ms,
    .retry_ms_table_count = LWS_ARRAY_SIZE(backoff_ms),
    .conceal_count = LWS_ARRAY_SIZE(backoff_ms),
    .secs_since_valid_ping = 5,
    .secs_since_valid_hangup = 10,
    .jitter_percent = 0,
};
#endif

// command line options
static const struct option options[] = {{"port", required_argument, NULL, 'p'},
                                        {"uid", required_argument, NULL, 'u'},
                                        {"gid", required_argument, NULL, 'g'},
                                        {"signal", required_argument, NULL, 's'},
                                        {"cwd", required_argument, NULL, 'w'},
#if LWS_LIBRARY_VERSION_NUMBER >= 4000000
                                        {"ping-interval", required_argument, NULL, 'P'},
#endif
                                        {"terminal-type", required_argument, NULL, 'T'},
                                        {"client-option", required_argument, NULL, 't'},
                                        {"max-clients", required_argument, NULL, 'm'},
                                        {"debug", required_argument, NULL, 'd'},
                                        {"version", no_argument, NULL, 'v'},
                                        {"help", no_argument, NULL, 'h'},
                                        {NULL, 0, 0, 0}};
static const char *opt_string = "p:u:g:s:w:P:t:T:m:d:vh";

static void print_help() {
  // clang-format off
  fprintf(stderr, "ttyd is a tool for sharing terminal over the web\n\n"
          "USAGE:\n"
          "    ttyd [options] <command> [<arguments...>]\n\n"
          "VERSION:\n"
          "    %s\n\n"
          "OPTIONS:\n"
          "    -p, --port              Port to listen (default: 7681, use `0` for random port)\n"
          "    -u, --uid               User id to run with\n"
          "    -g, --gid               Group id to run with\n"
          "    -s, --signal            Signal to send to the command when exit it (default: 1, SIGHUP)\n"
          "    -w, --cwd               Working directory to be set for the child program\n"
          "    -t, --client-option     Send option to client (format: key=value), repeat to add more options\n"
          "    -T, --terminal-type     Terminal type to report, default: xterm-256color\n"
          "    -m, --max-clients       Maximum clients to support (default: 0, no limit)\n"
#if LWS_LIBRARY_VERSION_NUMBER >= 4000000
          "    -P, --ping-interval     Websocket ping interval(sec) (default: 5)\n"
#endif
          "    -d, --debug             Set log level (default: 7)\n"
          "    -v, --version           Print the version and exit\n"
          "    -h, --help              Print this text and exit\n\n"
          "Visit https://github.com/tsl0922/ttyd to get more information and report bugs.\n",
          TTYD_VERSION
  );
  // clang-format on
}

static void print_config() {
  lwsl_notice("tty configuration:\n");
  lwsl_notice("  start command: %s\n", server->command);
  lwsl_notice("  close signal: %s (%d)\n", server->sig_name, server->sig_code);
  lwsl_notice("  terminal type: %s\n", server->terminal_type);
  if (server->max_clients > 0) lwsl_notice("  max clients: %d\n", server->max_clients);
  if (server->cwd != NULL) lwsl_notice("  working directory: %s\n", server->cwd);
}

static struct server *server_new(int argc, char **argv, int start) {
  struct server *ts;
  size_t cmd_len = 0;

  ts = xmalloc(sizeof(struct server));

  memset(ts, 0, sizeof(struct server));
  ts->client_count = 0;
  ts->sig_code = SIGHUP;
  snprintf(ts->terminal_type, sizeof(ts->terminal_type), "%s", "xterm-256color");
  get_sig_name(ts->sig_code, ts->sig_name, sizeof(ts->sig_name));
  if (start == argc) return ts;

  int cmd_argc = argc - start;
  char **cmd_argv = &argv[start];
  ts->argv = xmalloc(sizeof(char *) * (cmd_argc + 1));
  for (int i = 0; i < cmd_argc; i++) {
    ts->argv[i] = strdup(cmd_argv[i]);
    cmd_len += strlen(ts->argv[i]);
    if (i != cmd_argc - 1) {
      cmd_len++;  // for space
    }
  }
  ts->argv[cmd_argc] = NULL;
  ts->argc = cmd_argc;

  ts->command = xmalloc(cmd_len + 1);
  char *ptr = ts->command;
  for (int i = 0; i < cmd_argc; i++) {
    size_t len = strlen(ts->argv[i]);
    ptr = (char *)memcpy(ptr, ts->argv[i], len + 1) + len;
    if (i != cmd_argc - 1) {
      *ptr++ = ' ';
    }
  }
  *ptr = '\0';  // null terminator

  ts->loop = xmalloc(sizeof *ts->loop);
  uv_loop_init(ts->loop);

  return ts;
}

static void server_free(struct server *ts) {
  if (ts == NULL) return;
  if (ts->cwd != NULL) free(ts->cwd);
  free(ts->command);
  free(ts->prefs_json);

  char **p = ts->argv;
  for (; *p; p++) free(*p);
  free(ts->argv);

  uv_loop_close(ts->loop);

  free(ts->loop);
  free(ts);
}

static void signal_cb(uv_signal_t *watcher, int signum) {
  char sig_name[20];

  switch (watcher->signum) {
    case SIGINT:
    case SIGTERM:
      get_sig_name(watcher->signum, sig_name, sizeof(sig_name));
      lwsl_notice("received signal: %s (%d), exiting...\n", sig_name, watcher->signum);
      break;
    default:
      signal(SIGABRT, SIG_DFL);
      abort();
  }

  if (force_exit) exit(EXIT_FAILURE);
  force_exit = true;

  lws_cancel_service(context);
  uv_stop(server->loop);

  lwsl_notice("send ^C to force exit.\n");
}

static int parse_int(char *name, char *str) {
  char *endptr;
  errno = 0;
  long val = strtol(str, &endptr, 0);
  if (errno != 0 || endptr == str) {
    fprintf(stderr, "ttyd: invalid value for %s: %s\n", name, str);
    exit(EXIT_FAILURE);
  }
  return (int)val;
}

static int calc_command_start(int argc, char **argv) {
  // make a copy of argc and argv
  int argc_copy = argc;
  char **argv_copy = xmalloc(sizeof(char *) * argc);
  for (int i = 0; i < argc; i++) {
    argv_copy[i] = strdup(argv[i]);
  }

  // do not print error message for invalid option
  opterr = 0;
  while (getopt_long(argc_copy, argv_copy, opt_string, options, NULL) != -1)
    ;

  int start = argc;
  if (optind < argc) {
    char *command = argv_copy[optind];
    for (int i = 0; i < argc; i++) {
      if (strcmp(argv[i], command) == 0) {
        start = i;
        break;
      }
    }
  }

  // free argv copy
  for (int i = 0; i < argc; i++) {
    free(argv_copy[i]);
  }
  free(argv_copy);

  // reset for next use
  opterr = 1;
  optind = 0;

  return start;
}

int main(int argc, char **argv) {
  if (argc == 1) {
    print_help();
    return 0;
  }
  int start = calc_command_start(argc, argv);
  server = server_new(argc, argv, start);

  struct lws_context_creation_info info;
  memset(&info, 0, sizeof(info));
  info.port = 7681;
  info.iface = "127.0.0.1";
  info.protocols = protocols;
  info.gid = -1;
  info.uid = -1;
  info.max_http_header_pool = 16;
  info.options = LWS_SERVER_OPTION_LIBUV | LWS_SERVER_OPTION_VALIDATE_UTF8 | LWS_SERVER_OPTION_DISABLE_IPV6;
#ifndef LWS_WITHOUT_EXTENSIONS
  info.extensions = extensions;
#endif
  info.max_http_header_data = 65535;

  int debug_level = LLL_ERR | LLL_WARN | LLL_NOTICE;

  struct json_object *client_prefs = json_object_new_object();

  const char *home_dir = getenv("HOME");
  if (home_dir != NULL)
    json_object_object_add(client_prefs, "homeDir", json_object_new_string(home_dir));

  // parse command line options
  int c;
  while ((c = getopt_long(start, argv, opt_string, options, NULL)) != -1) {
    switch (c) {
      case 'h':
        print_help();
        return 0;
      case 'v':
        printf("ttyd version %s\n", TTYD_VERSION);
        return 0;
      case 'd':
        debug_level = parse_int("debug", optarg);
        break;
      case 'm':
        server->max_clients = parse_int("max-clients", optarg);
        break;
      case 'p':
        info.port = parse_int("port", optarg);
        if (info.port < 0) {
          fprintf(stderr, "ttyd: invalid port: %s\n", optarg);
          return -1;
        }
        break;
      case 'u':
        info.uid = parse_int("uid", optarg);
        break;
      case 'g':
        info.gid = parse_int("gid", optarg);
        break;
      case 's': {
        int sig = get_sig(optarg);
        if (sig > 0) {
          server->sig_code = sig;
          get_sig_name(sig, server->sig_name, sizeof(server->sig_name));
        } else {
          fprintf(stderr, "ttyd: invalid signal: %s\n", optarg);
          return -1;
        }
      } break;
      case 'w':
        server->cwd = strdup(optarg);
        break;
#if LWS_LIBRARY_VERSION_NUMBER >= 4000000
      case 'P': {
        int interval = parse_int("ping-interval", optarg);
        if (interval < 0) {
          fprintf(stderr, "ttyd: invalid ping interval: %s\n", optarg);
          return -1;
        }
        retry.secs_since_valid_ping = interval;
        retry.secs_since_valid_hangup = interval + 7;
      } break;
#endif
      case 'T':
        strncpy(server->terminal_type, optarg, sizeof(server->terminal_type) - 1);
        server->terminal_type[sizeof(server->terminal_type) - 1] = '\0';
        break;
      case '?':
        break;
      case 't':
        optind--;
        for (; optind < start && *argv[optind] != '-'; optind++) {
          char *option = optarg;
          char *key = strsep(&option, "=");
          if (key == NULL) {
            fprintf(stderr, "ttyd: invalid client option: %s, format: key=value\n", optarg);
            return -1;
          }
          char *value = strsep(&option, "=");
          if (value == NULL) {
            fprintf(stderr, "ttyd: invalid client option: %s, format: key=value\n", optarg);
            return -1;
          }
          struct json_object *obj = json_tokener_parse(value);
          json_object_object_add(client_prefs, key, obj != NULL ? obj : json_object_new_string(value));
        }
        break;
      default:
        print_help();
        return -1;
    }
  }
  server->prefs_json = strdup(json_object_to_json_string(client_prefs));
  json_object_put(client_prefs);

  if (server->command == NULL || strlen(server->command) == 0) {
    fprintf(stderr, "ttyd: missing start command\n");
    return -1;
  }

  lws_set_log_level(debug_level, NULL);

  char server_hdr[128] = "";
  snprintf(server_hdr, sizeof(server_hdr), "ttyd/%s (libwebsockets/%s)", TTYD_VERSION, LWS_LIBRARY_VERSION);
  info.server_string = server_hdr;

#if LWS_LIBRARY_VERSION_NUMBER < 4000000
  info.ws_ping_pong_interval = 5;
#else
  info.retry_and_idle_policy = &retry;
#endif

  lwsl_notice("ttyd %s (libwebsockets %s)\n", TTYD_VERSION, LWS_LIBRARY_VERSION);
  print_config();

  void *foreign_loops[1];
  foreign_loops[0] = server->loop;
  info.foreign_loops = foreign_loops;
  info.options |= LWS_SERVER_OPTION_EXPLICIT_VHOSTS;

  context = lws_create_context(&info);
  if (context == NULL) {
    lwsl_err("libwebsockets context creation failed\n");
    return 1;
  }

  struct lws_vhost *vhost = lws_create_vhost(context, &info);
  if (vhost == NULL) {
    lwsl_err("libwebsockets vhost creation failed\n");
    return 1;
  }
  int port = lws_get_vhost_listen_port(vhost);
  lwsl_notice(" Listening on port: %d\n", port);

#define sig_count 2
  int sig_nums[] = {SIGINT, SIGTERM};
  uv_signal_t signals[sig_count];
  for (int i = 0; i < sig_count; i++) {
    uv_signal_init(server->loop, &signals[i]);
    uv_signal_start(&signals[i], signal_cb, sig_nums[i]);
  }

  lws_service(context, 0);

  for (int i = 0; i < sig_count; i++) {
    uv_signal_stop(&signals[i]);
  }
#undef sig_count

  lws_context_destroy(context);

  // cleanup
  server_free(server);

  return 0;
}
