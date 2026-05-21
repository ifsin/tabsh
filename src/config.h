#pragma once
#include <stdbool.h>
#include <stddef.h>

#define MAX_APP_ARGS 32
#define MAX_APP_ENV  32
#define MAX_APPS     64

struct app_theme {
    char foreground[32];
    char background[32];
    char cursor[32];
    char cursor_style[16];
    bool cursor_blink;
    int  font_size;
    char font_family[256];
    char palette[16][32];

    bool has_foreground, has_background, has_cursor, has_cursor_style;
    bool has_cursor_blink, has_font_size, has_font_family;
    bool has_palette[16];
};

struct app_entry {
    char             id[64];
    char             name[128];
    char             command[512];
    char            *args[MAX_APP_ARGS];
    int              argc;
    char            *env_keys[MAX_APP_ENV];
    char            *env_vals[MAX_APP_ENV];
    int              envc;
    char             cwd[512];
    char             icon[512];
    struct app_theme theme;
};

struct tabsh_config {
    int              port;
    int              max_clients;
    int              sig_code;
    char             terminal_type[30];
    int              debug;
    bool             loaded;
    struct app_theme theme;
    struct app_entry apps[MAX_APPS];
    int              app_count;
};

extern struct tabsh_config *g_config;

int               config_load(const char *explicit_path);
void              config_set_legacy(const char **argv, int argc);
struct app_entry *config_get_app(const char *id);
struct app_entry *config_get_first_app(void);
void              config_resolve_theme(const struct app_entry *app, struct app_theme *out);
void              config_free(void);
