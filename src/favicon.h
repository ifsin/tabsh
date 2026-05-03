#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <uv.h>

#include "server.h"

// active connection tracking (used by protocol.c and favicon.c)
void favicon_pss_add(struct pss_tty *pss);
void favicon_pss_remove(struct pss_tty *pss);
bool favicon_pss_check(struct pss_tty *pss);

// resolve brew formula name from app argv string
bool favicon_resolve_formula(const char *app, char *formula, size_t formula_len);

// queue a background fetch of the favicon for the given formula
void favicon_queue_fetch(struct pss_tty *pss, const char *formula, const char *cache_path, const char *none_path);
