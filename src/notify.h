#pragma once

typedef struct notify_ctx notify_ctx_t;

void notify_ctx_init(notify_ctx_t **ctx, const char *session_id);
void notify_ctx_destroy(notify_ctx_t *ctx);
const char *notify_shim_dir(notify_ctx_t *ctx);
const char *notify_init_dir(notify_ctx_t *ctx);
