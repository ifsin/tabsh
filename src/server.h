#pragma once

#include <libwebsockets.h>
#include <stdbool.h>
#include <uv.h>

#include "notify.h"
#include "pty.h"
#include "compat.h"

#define PTY_RING_SIZE (256 * 1024)

// client → server
#define INPUT    '0'
#define RESIZE   '1'
#define PAUSE    '2'
#define RESUME   '3'
#define QUIT     '4'
#define CLEAR    '5'
#define JSON_DATA '{'

// server → client
#define CELL_DIFF    '0'
#define SB_PUSH      '1'
#define WINDOW_TITLE '2'
#define PREFERENCES  '3'
#define REATTACHED   '4'
#define APP_COMMAND  '5'
#define APP_FAVICON  '6'
#define MOUSE_MODE   '7'
#define ALT_SCREEN   '8'
#define CURSOR_BLINK '9'

// url paths
struct endpoints {
  char *ws;
  char *index;
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
  char address[50];
  char path[128];
  char session_id[SESSION_ID_LEN];

  struct lws *wsi;
  char *buffer;
  size_t len;

  pty_process *process;
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

  bool pending_frame;

  bool pending_altscreen_send;
  uint8_t pending_altscreen_value;

  bool pending_mouse_mode_send;
  uint8_t pending_mouse_mode_value;

  bool pending_cursor_blink_send;
  bool pending_cursor_blink_value;
};

struct server {
  int client_count;        // client count
  char *prefs_json;        // client preferences
  char *command;           // full command line
  char **argv;             // command with arguments
  int argc;                // command + arguments count
  char *cwd;               // working directory
  int sig_code;            // close signal
  char sig_name[20];       // human readable signal string
  int max_clients;         // maximum clients to support
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
