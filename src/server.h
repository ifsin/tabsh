#pragma once

#include <libwebsockets.h>
#include <stdbool.h>
#include <uv.h>

#include "notify.h"
#include "pty.h"
#include "compat.h"

#define PTY_RING_SIZE (256 * 1024)

// client message
#define INPUT '0'
#define RESIZE_TERMINAL '1'
#define PAUSE '2'
#define RESUME '3'
#define QUIT '4'
#define JSON_DATA '{'

// server message
#define OUTPUT '0'
#define SET_WINDOW_TITLE '1'
#define SET_PREFERENCES '2'
#define SET_APP_COMMAND '3'
#define SET_REATTACHED '4'
#define SET_APP_FAVICON '5'
#define CELL_DIFF '6'
#define SB_PUSH '7'
#define MOUSE_MODE '8'

// url paths
struct endpoints {
  char *ws;
  char *index;
  char *token;
  char *parent;
  char *Bell;
  char *favicon;
  char *content;
};

extern volatile bool force_exit;
extern struct lws_context *context;
extern struct server *server;
extern struct endpoints endpoints;

struct pss_http {
  char path[128];
  char *buffer;
  char *ptr;
  size_t len;
};

#define SESSION_TIMEOUT_MS 10000
#define SESSION_ID_LEN 37
#define APP_COMMAND_LEN 4096

struct pss_tty;
struct terminal_s;

typedef struct session {
  char id[SESSION_ID_LEN];
  pty_process *process;
  struct pss_tty *pss;
  uv_timer_t *timer;
  pid_t root_pid;
  bool detached;
  struct session *next;
  notify_ctx_t *notify;
  struct terminal_s *terminal;
} session_t;

struct pss_tty {
  bool initialized;
  int initial_cmd_index;
  bool authenticated;
  char user[30];
  char address[50];
  char path[128];
  char **args;
  int argc;
  char session_id[SESSION_ID_LEN];

  struct lws *wsi;
  char *buffer;
  size_t len;

  pty_process *process;
  pty_buf_t *pty_buf;
  session_t *session;

  char *cwd;
  char *app_command;
  int lws_close_status;
  bool intentional_close;
  bool reattached;

  char current_app[APP_COMMAND_LEN];
  char pending_app[APP_COMMAND_LEN];
  bool pending_app_send;

  char current_favicon_formula[256];
  char pending_favicon[256];
  bool pending_favicon_send;

  notify_ctx_t *notify;

  pid_t last_fgpid;

  bool cell_diff_enabled;
  bool pending_frame;
};

struct server {
  int client_count;        // client count
  char *prefs_json;        // client preferences
  char *credential;        // encoded basic auth credential
  char *auth_header;       // header name used for auth proxy
  char *index;             // custom index.html
  char *Bell;              // custom Bull.mp3
  char *command;           // full command line
  char **argv;             // command with arguments
  int argc;                // command + arguments count
  char *cwd;               // working directory
  int sig_code;            // close signal
  char sig_name[20];       // human readable signal string
  bool url_arg;            // allow client to send cli arguments in URL
  bool writable;           // whether clients to write to the TTY
  bool check_origin;       // whether allow websocket connection from different origin
  int max_clients;         // maximum clients to support
  bool once;               // whether accept only one client and exit on disconnection
  bool exit_no_conn;       // whether exit on all clients disconnection
  char socket_path[255];   // UNIX domain socket path
  char terminal_type[30];  // terminal type to report

  uv_loop_t *loop;         // the libuv event loop
  session_t *sessions;     // linked list of detached sessions

  char pty_ring[PTY_RING_SIZE];
  size_t pty_ring_head;
  size_t pty_ring_len;
};

session_t *session_find(const char *id);
session_t *session_create(const char *id, pty_process *process, struct pss_tty *pss);
void session_detach(session_t *session);
void session_attach(session_t *session, struct pss_tty *pss);
void session_remove(session_t *session);
void session_stop(session_t *session);
