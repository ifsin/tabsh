#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
#ifndef _WIN32
#include <ftw.h>
#endif

#include "notify.h"
#include "server.h"
#include "utils.h"
#include "shims.h"

#ifndef _WIN32

struct notify_ctx {
  char shim_dir[256];
  char init_dir[256];
};

static int write_file(const char *path, const char *content, size_t len) {
  FILE *f = fopen(path, "w");
  if (f == NULL) return -1;
  fwrite(content, 1, len, f);
  fclose(f);
  return chmod(path, 0755);
}

static int rm_cb(const char *pathname, const struct stat *sbuf, int type, struct FTW *ftwb) {
  (void)sbuf; (void)type; (void)ftwb;
  remove(pathname);
  return 0;
}

static void rm_rf(const char *dir) {
  nftw(dir, rm_cb, 64, FTW_DEPTH | FTW_PHYS);
}

void notify_ctx_init(notify_ctx_t **ctx, const char *session_id) {
  *ctx = xmalloc(sizeof(notify_ctx_t));
  memset(*ctx, 0, sizeof(notify_ctx_t));
  notify_ctx_t *c = *ctx;

  snprintf(c->shim_dir, sizeof(c->shim_dir), "/tmp/tabsh-shims-%s", session_id);
  snprintf(c->init_dir, sizeof(c->init_dir), "/tmp/tabsh-init-%s", session_id);

  mkdir(c->shim_dir, 0700);
  mkdir(c->init_dir, 0700);

  char path[512];

#define WRITE_SHIM(name, var) \
  snprintf(path, sizeof(path), "%s/" #name, c->shim_dir); \
  write_file(path, var, var##_len)
  WRITE_SHIM(notify-send, shim_notify_send);
  WRITE_SHIM(osascript, shim_osascript);
  WRITE_SHIM(terminal-notifier, shim_terminal_notifier);
  WRITE_SHIM(kdialog, shim_kdialog);
#undef WRITE_SHIM

#define WRITE_INIT(file, var) \
  snprintf(path, sizeof(path), "%s/" file, c->init_dir); \
  write_file(path, var, var##_len)
  WRITE_INIT("bashrc", init_bashrc);
  snprintf(path, sizeof(path), "%s/zsh", c->init_dir); mkdir(path, 0700);
  snprintf(path, sizeof(path), "%s/zsh/.zshrc", c->init_dir);
  write_file(path, init_zshrc, init_zshrc_len);
  snprintf(path, sizeof(path), "%s/fish", c->init_dir); mkdir(path, 0700);
  snprintf(path, sizeof(path), "%s/fish/fish", c->init_dir); mkdir(path, 0700);
  snprintf(path, sizeof(path), "%s/fish/fish/config.fish", c->init_dir);
  write_file(path, init_fish_config, init_fish_config_len);
  snprintf(path, sizeof(path), "%s/fish/fish/conf.d", c->init_dir); mkdir(path, 0700);
  snprintf(path, sizeof(path), "%s/fish/fish/conf.d/zzz-tabsh.fish", c->init_dir);
  write_file(path, init_fish_conf, init_fish_conf_len);
  WRITE_INIT("env.sh", init_envsh);
  WRITE_INIT("init.ps1", init_pwsh);
  WRITE_INIT("init.bat", init_bat);
#undef WRITE_INIT
}

void notify_ctx_destroy(notify_ctx_t *ctx) {
  if (ctx == NULL) return;
  rm_rf(ctx->shim_dir);
  rm_rf(ctx->init_dir);
  free(ctx);
}

const char *notify_shim_dir(notify_ctx_t *ctx) {
  return ctx ? ctx->shim_dir : NULL;
}

const char *notify_init_dir(notify_ctx_t *ctx) {
  return ctx ? ctx->init_dir : NULL;
}

#else /* _WIN32 */

void notify_ctx_init(notify_ctx_t **ctx, const char *session_id) {
  (void)ctx; (void)session_id;
}
void notify_ctx_destroy(notify_ctx_t *ctx) { (void)ctx; }
const char *notify_shim_dir(notify_ctx_t *ctx) { (void)ctx; return NULL; }
const char *notify_init_dir(notify_ctx_t *ctx) { (void)ctx; return NULL; }

#endif /* !_WIN32 */
