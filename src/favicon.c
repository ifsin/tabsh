#include <json.h>
#include <libwebsockets.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#ifndef _WIN32
#include <unistd.h>
#endif
#include "utils.h"

#include "favicon.h"
#include "server.h"
#include "utils.h"

// ---------------------------------------------------------------------------
// active connection tracking
// ---------------------------------------------------------------------------

typedef struct active_pss_node {
    struct pss_tty         *pss;
    struct active_pss_node *next;
} active_pss_node_t;

static active_pss_node_t *active_pss_head = NULL;

void favicon_pss_add(struct pss_tty *pss) {
    active_pss_node_t *node = xmalloc(sizeof(active_pss_node_t));
    node->pss       = pss;
    node->next      = active_pss_head;
    active_pss_head = node;
}

void favicon_pss_remove(struct pss_tty *pss) {
    active_pss_node_t **prev = &active_pss_head;
    while (*prev) {
        if ((*prev)->pss == pss) {
            active_pss_node_t *del = *prev;
            *prev = del->next;
            free(del);
            return;
        }
        prev = &(*prev)->next;
    }
}

bool favicon_pss_check(struct pss_tty *pss) {
    for (active_pss_node_t *n = active_pss_head; n; n = n->next)
        if (n->pss == pss) return true;
    return false;
}

// ---------------------------------------------------------------------------
// formula resolution
// ---------------------------------------------------------------------------

static const char *shell_names[] = { "zsh", "bash", "fish", "sh", "dash", "ksh", "tcsh", NULL };

static bool is_shell(const char *name) {
    for (int i = 0; shell_names[i]; i++)
        if (strcmp(name, shell_names[i]) == 0) return true;
    return false;
}

#ifndef _WIN32
static bool resolve_binary_formula(const char *binary, char *formula, size_t formula_len) {
    const char *brew_bins[] = { "/opt/homebrew/bin/", "/usr/local/bin/", NULL };
    for (int i = 0; brew_bins[i]; i++) {
        char    link_path[512];
        snprintf(link_path, sizeof(link_path), "%s%s", brew_bins[i], binary);
        char    target[1024];
        ssize_t n = readlink(link_path, target, sizeof(target) - 1);
        if (n <= 0) continue;
        target[n] = '\0';

        char  *seg  = strstr(target, "/Cellar/");
        size_t skip = 8;
        if (!seg) { seg = strstr(target, "/Caskroom/"); skip = 10; }
        if (!seg) { seg = strstr(target, "/node_modules/"); skip = 14; }
        if (!seg) continue;
        seg += skip;
        char  *slash = strchr(seg, '/');
        if (!slash) continue;
        size_t flen = (size_t)(slash - seg);
        if (flen >= formula_len) continue;
        strncpy(formula, seg, flen);
        formula[flen] = '\0';

        return true;
    }
    return false;
}
#endif /* !_WIN32 */

bool favicon_resolve_formula(const char *app, char *formula, size_t formula_len) {
    char buf[512];
    strncpy(buf, app, sizeof(buf) - 1);
    buf[sizeof(buf) - 1] = '\0';

    // bail early if argv[0] is a shell — returning to prompt should clear favicon
    const char *argv0 = buf;
    const char *slash = strrchr(buf, '/');
    if (slash) argv0 = slash + 1;
    char        name0[64] = { 0 };
    const char *sp        = strchr(argv0, ' ');
    size_t      nlen      = sp ? (size_t)(sp - argv0) : strlen(argv0);
    if (nlen < sizeof(name0)) { strncpy(name0, argv0, nlen); name0[nlen] = '\0'; }
    if (is_shell(name0)) return false;

    char *tokens[32];
    int   ntokens = 0;
    char *tok     = strtok(buf, " ");
    while (tok && ntokens < 32) {
        tokens[ntokens++] = tok;
        tok               = strtok(NULL, " ");
    }
    if (ntokens == 0) return false;

    // pass 1: path tokens right to left (explicit binary paths, most specific)
    for (int i = ntokens - 1; i >= 0; i--) {
        if (strchr(tokens[i], '/') == NULL) continue;
        const char *name = strrchr(tokens[i], '/') + 1;
#ifndef _WIN32
        if (resolve_binary_formula(name, formula, formula_len)) return true;
#else
        (void)name;
#endif
    }

    // pass 2: fall back to argv[0]
    const char *base = strrchr(tokens[0], '/');
    const char *name = base ? base + 1 : tokens[0];
#ifndef _WIN32
    if (resolve_binary_formula(name, formula, formula_len)) return true;
#else
    (void)name;
#endif

    return false;
}

#ifndef _WIN32
// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

// follow HTTP redirects, write the final url_effective into out
static void follow_http_redirects(const char *url, char *out, size_t out_len) {
    char cmd[768];
    snprintf(cmd, sizeof(cmd),
             "curl -sL --max-time 5 -o /dev/null -w '%%{url_effective}' '%s' 2>/dev/null", url);
    FILE  *fp = popen(cmd, "r");
    if (!fp) { strncpy(out, url, out_len - 1); out[out_len - 1] = '\0'; return; }
    size_t n = fread(out, 1, out_len - 1, fp);
    pclose(fp);
    out[n] = '\0';
    if (out[0] == '\0') strncpy(out, url, out_len - 1);
}

// fetch page body and extract meta-refresh URL if present, returns true if found
static bool get_meta_refresh(const char *url, char *out, size_t out_len) {
    char  cmd[768];
    snprintf(cmd, sizeof(cmd), "curl -sL --max-time 5 '%s' 2>/dev/null", url);
    FILE *fp = popen(cmd, "r");
    if (!fp) return false;
    char  html[8192] = { 0 };
    fread(html, 1, sizeof(html) - 1, fp);
    pclose(fp);

    // look for: http-equiv=refresh ... content="...URL..."  (case-insensitive, attr order varies)
    char *p = html;
    while (*p) {
        char *eq = strcasestr(p, "http-equiv");
        if (!eq) break;
        // find the enclosing tag boundary (scan up to 512 chars for the full tag)
        char *tag_start = eq;
        while (tag_start > html && *tag_start != '<') tag_start--;
        char  tag[512] = { 0 };
        strncpy(tag, tag_start, sizeof(tag) - 1);
        char *tag_end = strchr(tag, '>');
        if (tag_end) *tag_end = '\0';

        if (strcasestr(tag, "refresh")) {
            // extract URL from content="0; url" or content="0; https://..."
            char *content = strcasestr(tag, "content=");
            if (content) {
                content += 8;
                if (*content == '"' || *content == '\'') content++;
                // skip past the delay number and separator
                char *url_part = strcasestr(content, "url=");
                if (!url_part) url_part = strchr(content, ';');
                if (url_part) {
                    if (strncasecmp(url_part, "url=", 4) == 0) url_part += 4;
                    else url_part++;
                    while (*url_part == ' ') url_part++;
                    char  *end = strpbrk(url_part, "\"' \t\n\r>");
                    size_t len = end ? (size_t)(end - url_part) : strlen(url_part);
                    if (len > 0 && len < out_len) {
                        strncpy(out, url_part, len);
                        out[len] = '\0';
                        return true;
                    }
                } else {
                    // no url= prefix, the content after the delay is the URL
                    char *semi = strchr(content, ';');
                    if (!semi) semi = strchr(content, ',');
                    if (semi) {
                        semi++;
                        while (*semi == ' ') semi++;
                        char  *end = strpbrk(semi, "\"' \t\n\r>");
                        size_t len = end ? (size_t)(semi - end) : strlen(semi);
                        if (len > 0 && len < out_len) {
                            strncpy(out, semi, len);
                            out[len] = '\0';
                            return true;
                        }
                    }
                }
            }
        }
        p = eq + 1;
    }
    return false;
}

// extract a non-empty quoted string value for "key": from JSON buf
static bool json_extract_string(const char *buf, const char *key, char *out, size_t out_len) {
    char  search[128];
    snprintf(search, sizeof(search), "\"%s\":", key);
    char *p = strstr(buf, search);
    if (!p) return false;
    p += strlen(search);
    while (*p == ' ') p++;
    if (*p != '"') return false;
    p++;
    char  *end = strchr(p, '"');
    if (!end || p == end) return false;
    size_t len = (size_t)(end - p);
    if (len >= out_len) return false;
    strncpy(out, p, len);
    out[len] = '\0';
    return true;
}

// for a github.com/<user>/<repo> URL, resolve a favicon URL via the repo API:
// uses homepage if set, otherwise falls back to the owner's avatar_url.
static bool resolve_github_homepage(const char *url, char *out, size_t out_len) {
    const char *prefix = "https://github.com/";
    if (strncmp(url, prefix, strlen(prefix)) != 0) return false;

    const char *rest  = url + strlen(prefix);
    const char *slash = strchr(rest, '/');
    if (!slash) return false;

    char   user[128] = { 0 };
    size_t ulen      = (size_t)(slash - rest);
    if (ulen >= sizeof(user)) return false;
    strncpy(user, rest, ulen);

    const char *repo_start = slash + 1;
    const char *repo_end   = strchr(repo_start, '/');
    char        repo[128]  = { 0 };
    size_t      rlen       = repo_end ? (size_t)(repo_end - repo_start) : strlen(repo_start);
    if (rlen == 0 || rlen >= sizeof(repo)) return false;
    strncpy(repo, repo_start, rlen);

    char api_url[512];
    snprintf(api_url, sizeof(api_url), "https://api.github.com/repos/%s/%s", user, repo);

    char cmd[768];
    snprintf(cmd, sizeof(cmd),
             "curl -sL --max-time 5 -H 'User-Agent: ttyd' '%s' 2>/dev/null", api_url);
    FILE  *fp = popen(cmd, "r");
    if (!fp) return false;
    char   buf[16384] = { 0 };
    size_t n          = fread(buf, 1, sizeof(buf) - 1, fp);
    pclose(fp);
    buf[n] = '\0';

    // prefer repo homepage
    char homepage[512] = { 0 };
    if (json_extract_string(buf, "homepage", homepage, sizeof(homepage)) && homepage[0] != '\0') {
        strncpy(out, homepage, out_len - 1);
        out[out_len - 1] = '\0';
        return true;
    }

    // fallback: owner avatar — only for organisations, not individual users
    char *owner = strstr(buf, "\"owner\":");
    if (owner) {
        char owner_type[64] = { 0 };
        json_extract_string(owner, "type", owner_type, sizeof(owner_type));
        if (strcmp(owner_type, "Organization") == 0) {
            char avatar[512] = { 0 };
            if (json_extract_string(owner, "avatar_url", avatar, sizeof(avatar)) && avatar[0] != '\0') {
                strncpy(out, avatar, out_len - 1);
                out[out_len - 1] = '\0';
                return true;
            }
        }
    }

    return false;
}

// ---------------------------------------------------------------------------
// async fetch
// ---------------------------------------------------------------------------

typedef struct {
    uv_work_t       work;
    char            formula[256];
    char            cache_path[512];
    char            none_path[512];
    struct pss_tty *pss;
    bool            success;
} favicon_work_t;

static void favicon_fetch_work(uv_work_t *req) {
    favicon_work_t *w = (favicon_work_t *)req;

    // get homepage from brew info
    char   cmd[512];
    snprintf(cmd, sizeof(cmd), "brew info --json=v2 '%s' 2>/dev/null", w->formula);
    FILE  *fp = popen(cmd, "r");
    if (!fp) return;
    char   buf[65536];
    size_t n = fread(buf, 1, sizeof(buf) - 1, fp);
    pclose(fp);
    buf[n] = '\0';

    char *hp = strstr(buf, "\"homepage\":");
    if (!hp) return;
    hp += 11;
    while (*hp == ' ' || *hp == '"') hp++;
    char *hpend = strchr(hp, '"');
    if (!hpend || hp == hpend) return;
    *hpend = '\0';


    // resolve URL chain: follow redirects, handle github, repeat until stable
    char favicon_url[512];
    strncpy(favicon_url, hp, sizeof(favicon_url) - 1);
    favicon_url[sizeof(favicon_url) - 1] = '\0';

    for (int depth = 0; depth < 8; depth++) {
        // step 1: follow HTTP redirects
        char next[512] = { 0 };
        follow_http_redirects(favicon_url, next, sizeof(next));

        // step 2: if page does a meta-refresh, follow that too
        char meta[512] = { 0 };
        if (get_meta_refresh(next, meta, sizeof(meta)) && meta[0] != '\0') {
            strncpy(favicon_url, meta, sizeof(favicon_url) - 1);
            favicon_url[sizeof(favicon_url) - 1] = '\0';
            continue;
        }

        // step 3: if it's a github repo, resolve via API
        if (strstr(next, "github.com/") != NULL) {
            char gh_result[512] = { 0 };
            if (resolve_github_homepage(next, gh_result, sizeof(gh_result))) {
                strncpy(favicon_url, gh_result, sizeof(favicon_url) - 1);
                favicon_url[sizeof(favicon_url) - 1] = '\0';
                if (strstr(gh_result, "avatars.githubusercontent.com") != NULL) break;
                continue;
            } else {
                return;
            }
        }


        strncpy(favicon_url, next, sizeof(favicon_url) - 1);
        break;
    }
    favicon_url[sizeof(favicon_url) - 1] = '\0';

    if (strstr(favicon_url, "://") == NULL) return;

    mkdir_p("/tmp/ttyd-favicons", 0755);

    char curl_cmd[1024];
    // avatars.githubusercontent.com URLs are direct images — download them as-is
    if (strstr(favicon_url, "avatars.githubusercontent.com") != NULL) {
        snprintf(curl_cmd, sizeof(curl_cmd),
                 "curl -sL --max-time 5 '%s' -o '%s' 2>/dev/null",
                 favicon_url, w->cache_path);
    } else {
        snprintf(curl_cmd, sizeof(curl_cmd),
                 "curl -sL --max-time 5 "
                 "'https://www.google.com/s2/favicons?domain=%s&sz=64' -o '%s' 2>/dev/null",
                 favicon_url, w->cache_path);
    }
    w->success = (system(curl_cmd) == 0);
}

static void favicon_fetch_done(uv_work_t *req, int status) {
    favicon_work_t *w = (favicon_work_t *)req;
    if (w->success) {
        if (favicon_pss_check(w->pss) && w->pss->wsi != NULL) {
            char favicon_url[512];
            snprintf(favicon_url, sizeof(favicon_url), "%s%s.png", endpoints.favicon, w->formula);
            lwsl_notice("[favicon] %s -> %s\n", w->formula, favicon_url);
            if (!w->pss->pending_state) w->pss->pending_state = json_object_new_object();
            json_object_object_add(w->pss->pending_state, "favicon", json_object_new_string(favicon_url));
            lws_callback_on_writable(w->pss->wsi);
        }
    } else {
        lwsl_notice("[favicon] %s: fetch failed, caching as none\n", w->formula);
        mkdir_p("/tmp/ttyd-favicons", 0755);
        FILE *f = fopen(w->none_path, "w");
        if (f) fclose(f);
    }
    free(w);
}

void favicon_queue_fetch(struct pss_tty *pss, const char *formula, const char *cache_path, const char *none_path) {
    favicon_work_t *w = xmalloc(sizeof(favicon_work_t));
    memset(w, 0, sizeof(*w));
    strncpy(w->formula, formula, sizeof(w->formula) - 1);
    strncpy(w->cache_path, cache_path, sizeof(w->cache_path) - 1);
    strncpy(w->none_path, none_path, sizeof(w->none_path) - 1);
    w->pss = pss;
    uv_queue_work(server->loop, &w->work, favicon_fetch_work, favicon_fetch_done);
}
#else
void favicon_queue_fetch(struct pss_tty *pss, const char *formula, const char *cache_path, const char *none_path) {
    (void)pss; (void)formula; (void)cache_path; (void)none_path;
}
#endif /* !_WIN32 */
