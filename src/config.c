#include "config.h"

#include "utils.h"

#include <errno.h>
#include <json.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

struct tabsh_config *g_config = NULL;

static struct tabsh_config g_config_storage;

/* Strip // line comments outside of strings (simple heuristic).
 * Returns a newly allocated string that caller must free. */
static char *strip_comments(const char *src, size_t len) {
    char *out = malloc(len + 1);
    if (!out) return NULL;
    size_t i = 0, o = 0;
    bool   in_str = false;
    while (i < len) {
        char c = src[i];
        if (in_str) {
            if (c == '\\' && i + 1 < len) {
                out[o++] = c;
                out[o++] = src[i + 1];
                i += 2;
                continue;
            }
            if (c == '"') in_str = false;
            out[o++] = c;
            i++;
        } else {
            if (c == '"') {
                in_str   = true;
                out[o++] = c;
                i++;
            } else if (c == '/' && i + 1 < len && src[i + 1] == '/') {
                /* replace rest of line with spaces */
                while (i < len && src[i] != '\n') {
                    out[o++] = ' ';
                    i++;
                }
            } else {
                out[o++] = c;
                i++;
            }
        }
    }
    out[o] = '\0';
    return out;
}

static char *read_file(const char *path, size_t *out_len) {
    FILE *f = fopen(path, "rb");
    if (!f) return NULL;
    fseek(f, 0, SEEK_END);
    long sz = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (sz <= 0) {
        fclose(f);
        return NULL;
    }
    char *buf = malloc((size_t)sz + 1);
    if (!buf) {
        fclose(f);
        return NULL;
    }
    size_t n = fread(buf, 1, (size_t)sz, f);
    fclose(f);
    buf[n] = '\0';
    if (out_len) *out_len = n;
    return buf;
}

static void parse_theme(struct json_object *obj, struct app_theme *t) {
    struct json_object *o;
    if (json_object_object_get_ex(obj, "foreground", &o)) {
        strncpy(t->foreground, json_object_get_string(o), sizeof(t->foreground) - 1);
        t->has_foreground = true;
    }
    if (json_object_object_get_ex(obj, "background", &o)) {
        strncpy(t->background, json_object_get_string(o), sizeof(t->background) - 1);
        t->has_background = true;
    }
    if (json_object_object_get_ex(obj, "cursor", &o)) {
        strncpy(t->cursor, json_object_get_string(o), sizeof(t->cursor) - 1);
        t->has_cursor = true;
    }
    if (json_object_object_get_ex(obj, "cursor_style", &o)) {
        strncpy(t->cursor_style, json_object_get_string(o), sizeof(t->cursor_style) - 1);
        t->has_cursor_style = true;
    }
    if (json_object_object_get_ex(obj, "cursor_blink", &o)) {
        t->cursor_blink     = json_object_get_boolean(o);
        t->has_cursor_blink = true;
    }
    if (json_object_object_get_ex(obj, "font_size", &o)) {
        t->font_size     = json_object_get_int(o);
        t->has_font_size = true;
    }
    if (json_object_object_get_ex(obj, "font_family", &o)) {
        strncpy(t->font_family, json_object_get_string(o), sizeof(t->font_family) - 1);
        t->has_font_family = true;
    }
    if (json_object_object_get_ex(obj, "palette", &o)) {
        int plen = json_object_array_length(o);
        for (int i = 0; i < plen && i < 16; i++) {
            struct json_object *pe = json_object_array_get_idx(o, i);
            if (pe) {
                strncpy(t->palette[i], json_object_get_string(pe), sizeof(t->palette[i]) - 1);
                t->has_palette[i] = true;
            }
        }
    }
}

static int parse_app(struct json_object *obj, struct app_entry *app) {
    struct json_object *o;

    if (!json_object_object_get_ex(obj, "id", &o)) {
        fprintf(stderr, "tabsh: app missing 'id' field\n");
        return -1;
    }
    strncpy(app->id, json_object_get_string(o), sizeof(app->id) - 1);

    if (!json_object_object_get_ex(obj, "command", &o)) {
        fprintf(stderr, "tabsh: app '%s' missing 'command' field\n", app->id);
        return -1;
    }
    strncpy(app->command, json_object_get_string(o), sizeof(app->command) - 1);

    if (json_object_object_get_ex(obj, "name", &o))
        strncpy(app->name, json_object_get_string(o), sizeof(app->name) - 1);
    else
        strncpy(app->name, app->id, sizeof(app->name) - 1);

    if (json_object_object_get_ex(obj, "cwd", &o)) {
        const char *s = json_object_get_string(o);
        if (s) strncpy(app->cwd, s, sizeof(app->cwd) - 1);
    }

    if (json_object_object_get_ex(obj, "icon", &o)) {
        const char *s = json_object_get_string(o);
        if (s) strncpy(app->icon, s, sizeof(app->icon) - 1);
    }

    if (json_object_object_get_ex(obj, "args", &o)) {
        int n = json_object_array_length(o);
        for (int i = 0; i < n && i < MAX_APP_ARGS; i++) {
            struct json_object *a = json_object_array_get_idx(o, i);
            if (a) {
                app->args[app->argc++] = strdup(json_object_get_string(a));
            }
        }
    }

    if (json_object_object_get_ex(obj, "env", &o)) {
        json_object_object_foreach(o, key, val) {
            if (app->envc < MAX_APP_ENV) {
                app->env_keys[app->envc] = strdup(key);
                app->env_vals[app->envc] = strdup(json_object_get_string(val));
                app->envc++;
            }
        }
    }

    if (json_object_object_get_ex(obj, "theme", &o)) parse_theme(o, &app->theme);

    return 0;
}

static int try_load(const char *path) {
    size_t len = 0;
    char  *raw = read_file(path, &len);
    if (!raw) return -1; /* not found */

    char *src = strip_comments(raw, len);
    free(raw);
    if (!src) {
        fprintf(stderr, "tabsh: out of memory\n");
        return -1;
    }

    struct json_object *root = json_tokener_parse(src);
    free(src);
    if (!root) {
        fprintf(stderr, "tabsh: failed to parse config: %s\n", path);
        return -1;
    }

    struct tabsh_config *cfg = g_config;
    struct json_object  *o;

    /* server block */
    if (json_object_object_get_ex(root, "server", &o)) {
        struct json_object *v;
        if (json_object_object_get_ex(o, "port", &v)) cfg->port = json_object_get_int(v);
        if (json_object_object_get_ex(o, "max_clients", &v)) cfg->max_clients = json_object_get_int(v);
        if (json_object_object_get_ex(o, "signal", &v)) cfg->sig_code = get_sig(json_object_get_string(v));
        if (json_object_object_get_ex(o, "terminal_type", &v)) {
            strncpy(cfg->terminal_type, json_object_get_string(v), sizeof(cfg->terminal_type) - 1);
            cfg->terminal_type[sizeof(cfg->terminal_type) - 1] = '\0';
        }
        if (json_object_object_get_ex(o, "debug", &v)) cfg->debug = json_object_get_int(v);
    }

    /* global theme */
    if (json_object_object_get_ex(root, "theme", &o)) parse_theme(o, &cfg->theme);

    /* apps array */
    if (json_object_object_get_ex(root, "apps", &o)) {
        int n = json_object_array_length(o);
        for (int i = 0; i < n && cfg->app_count < MAX_APPS; i++) {
            struct json_object *app_obj = json_object_array_get_idx(o, i);
            if (!app_obj) continue;
            struct app_entry *app = &cfg->apps[cfg->app_count];
            memset(app, 0, sizeof(*app));
            if (parse_app(app_obj, app) != 0) {
                json_object_put(root);
                return -1;
            }
            /* check duplicate ids */
            for (int j = 0; j < cfg->app_count; j++) {
                if (strcmp(cfg->apps[j].id, app->id) == 0) {
                    fprintf(stderr, "tabsh: duplicate app id: %s\n", app->id);
                    json_object_put(root);
                    return -1;
                }
            }
            cfg->app_count++;
        }
    }

    json_object_put(root);
    cfg->loaded = true;
    return 0;
}

int config_load(const char *explicit_path) {
    if (!g_config) {
        g_config = &g_config_storage;
        memset(g_config, 0, sizeof(*g_config));
    }

    if (explicit_path) {
        int r = try_load(explicit_path);
        if (r != 0) {
            fprintf(stderr, "tabsh: failed to load config: %s\n", explicit_path);
            return -1;
        }
        return 0;
    }

    /* check $TABSH_CONFIG */
    const char *env_path = getenv("TABSH_CONFIG");
    if (env_path && try_load(env_path) == 0) return 0;

    /* ./tabsh.json */
    if (try_load("./tabsh.json") == 0) return 0;

    /* ~/.config/tabsh/config.json */
    const char *home = getenv("HOME");
    if (home) {
        char path[1024];
        snprintf(path, sizeof(path), "%s/.config/tabsh/config.json", home);
        if (try_load(path) == 0) return 0;
    }

    /* no config found — return 0 with app_count=0, loaded=false */
    return 0;
}

void config_set_legacy(const char **argv, int argc) {
    if (!g_config) {
        g_config = &g_config_storage;
        memset(g_config, 0, sizeof(*g_config));
    }
    g_config->loaded      = false;
    struct app_entry *app = &g_config->apps[0];
    memset(app, 0, sizeof(*app));
    strncpy(app->id, "term", sizeof(app->id) - 1);
    strncpy(app->name, "Terminal", sizeof(app->name) - 1);
    strncpy(app->command, argv[0], sizeof(app->command) - 1);
    for (int i = 1; i < argc && app->argc < MAX_APP_ARGS; i++) {
        app->args[app->argc++] = strdup(argv[i]);
    }
    g_config->app_count = 1;
}

struct app_entry *config_get_app(const char *id) {
    if (!g_config || !id) return NULL;
    for (int i = 0; i < g_config->app_count; i++) {
        if (strcmp(g_config->apps[i].id, id) == 0) return &g_config->apps[i];
    }
    return NULL;
}

struct app_entry *config_get_first_app(void) {
    if (!g_config || g_config->app_count == 0) return NULL;
    return &g_config->apps[0];
}

void config_resolve_theme(const struct app_entry *app, struct app_theme *out) {
    if (!g_config || !out) return;
    /* start with global theme */
    *out = g_config->theme;
    if (!app) return;
    /* overlay per-app fields */
    const struct app_theme *t = &app->theme;
    if (t->has_foreground) {
        strncpy(out->foreground, t->foreground, sizeof(out->foreground) - 1);
        out->has_foreground = true;
    }
    if (t->has_background) {
        strncpy(out->background, t->background, sizeof(out->background) - 1);
        out->has_background = true;
    }
    if (t->has_cursor) {
        strncpy(out->cursor, t->cursor, sizeof(out->cursor) - 1);
        out->has_cursor = true;
    }
    if (t->has_cursor_style) {
        strncpy(out->cursor_style, t->cursor_style, sizeof(out->cursor_style) - 1);
        out->has_cursor_style = true;
    }
    if (t->has_cursor_blink) {
        out->cursor_blink     = t->cursor_blink;
        out->has_cursor_blink = true;
    }
    if (t->has_font_size) {
        out->font_size     = t->font_size;
        out->has_font_size = true;
    }
    if (t->has_font_family) {
        strncpy(out->font_family, t->font_family, sizeof(out->font_family) - 1);
        out->has_font_family = true;
    }
    for (int i = 0; i < 16; i++) {
        if (t->has_palette[i]) {
            strncpy(out->palette[i], t->palette[i], sizeof(out->palette[i]) - 1);
            out->has_palette[i] = true;
        }
    }
}

void config_free(void) {
    if (!g_config) return;
    for (int i = 0; i < g_config->app_count; i++) {
        struct app_entry *app = &g_config->apps[i];
        for (int j = 0; j < app->argc; j++)
            free(app->args[j]);
        for (int j = 0; j < app->envc; j++) {
            free(app->env_keys[j]);
            free(app->env_vals[j]);
        }
    }
    g_config = NULL;
}
