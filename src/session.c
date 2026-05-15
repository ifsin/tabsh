#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "pty.h"
#include "server.h"
#include "utils.h"

session_t *session_find(const char *id) {
  for (session_t *s = server->sessions; s != NULL; s = s->next) {
    if (strcmp(s->id, id) == 0) return s;
  }
  return NULL;
}

void session_remove(session_t *session) {
  session_t **prev = &server->sessions;
  while (*prev != NULL) {
    if (*prev == session) {
      *prev = session->next;
      return;
    }
    prev = &(*prev)->next;
  }
}

void session_stop(session_t *session) {
  session->detached = true;
  session_remove(session);
  if (session->timer != NULL) {
    uv_timer_stop(session->timer);
    uv_close((uv_handle_t *)session->timer, (uv_close_cb)free);
    session->timer = NULL;
  }
  pty_pause(session->process);
  lwsl_notice("session %s stopping, killing process pid: %d\n", session->id, session->process->pid);
  pty_kill(session->process, server->sig_code);
}


static void session_timer_cb(uv_timer_t *timer) {
  session_t *session = (session_t *)timer->data;
  session->timer = NULL;
  lwsl_notice("session %s expired", session->id);
  uv_close((uv_handle_t *)timer, (uv_close_cb)free);
  session_stop(session);
}

session_t *session_create(const char *id, pty_process *process, struct pss_tty *pss) {
  session_t *session = xmalloc(sizeof(session_t));
  memset(session, 0, sizeof(session_t));
  strncpy(session->id, id, SESSION_ID_LEN - 1);
  session->id[SESSION_ID_LEN - 1] = '\0';
  session->process = process;
  session->pss = pss;
  session->detached = false;
  session->timer = xmalloc(sizeof(uv_timer_t));
  uv_timer_init(server->loop, session->timer);
  session->timer->data = session;
  session->next = NULL;
  return session;
}

void session_detach(session_t *session) {
  session->detached = true;
  session->pss = NULL;
  pty_pause(session->process);
  session->next = server->sessions;
  server->sessions = session;
  uv_timer_start(session->timer, session_timer_cb, SESSION_TIMEOUT_MS, 0);
  lwsl_notice("session %s detached, timeout %dms\n", session->id, SESSION_TIMEOUT_MS);
}

void session_attach(session_t *session, struct pss_tty *pss) {
  session->detached = false;
  session->pss = pss;
  pty_resume(session->process);
  session_remove(session);
  uv_timer_stop(session->timer);
  pss->reattached = true;
  if (session->notify != NULL) {
    pss->notify = session->notify;
  }
  lwsl_notice("session %s reattached\n", session->id);
}
