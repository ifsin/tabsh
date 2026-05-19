#include <libwebsockets.h>
#include <ctype.h>
#include <stdlib.h>
#include <string.h>
#include <zlib.h>

#include "html.h"
#include "beep.h"
#include "server.h"
#include "utils.h"

static char *html_cache = NULL;
static size_t html_cache_len = 0;

static bool accept_gzip(struct lws *wsi) {
  char buf[256];
  int len = lws_hdr_copy(wsi, buf, sizeof(buf), WSI_TOKEN_HTTP_ACCEPT_ENCODING);
  return len > 0 && strstr(buf, "gzip") != NULL;
}

static bool uncompress_html(char **output, size_t *output_len) {
  if (html_cache == NULL || html_cache_len == 0) {
    z_stream stream;
    memset(&stream, 0, sizeof(stream));
    if (inflateInit2(&stream, 16 + 15) != Z_OK) return false;

    html_cache_len = index_html_size;
    html_cache = xmalloc(html_cache_len);

    stream.avail_in = index_html_len;
    stream.avail_out = html_cache_len;
    stream.next_in = (void *)index_html;
    stream.next_out = (void *)html_cache;

    int ret = inflate(&stream, Z_SYNC_FLUSH);
    inflateEnd(&stream);
    if (ret != Z_STREAM_END) {
      free(html_cache);
      html_cache = NULL;
      html_cache_len = 0;
      return false;
    }
  }

  *output = html_cache;
  *output_len = html_cache_len;

  return true;
}

static void pss_buffer_free(struct pss_http *pss) {
  if (pss->buffer != (char *)index_html && pss->buffer != html_cache) free(pss->buffer);
}

static void access_log(struct lws *wsi, const char *path) {
  char rip[50];

  lws_get_peer_simple(lws_get_network_wsi(wsi), rip, sizeof(rip));
  lwsl_notice("HTTP %s - %s\n", path, rip);
}

static void url_decode(char *s) {
  char *d = s;
  while (*s) {
    if (*s == '%' && isxdigit((unsigned char)s[1]) && isxdigit((unsigned char)s[2])) {
      char hex[3] = {s[1], s[2], 0};
      *d++ = (char)strtol(hex, NULL, 16);
      s += 3;
    } else if (*s == '+') {
      *d++ = ' ';
      s++;
    } else {
      *d++ = *s++;
    }
  }
  *d = '\0';
}

int callback_http(struct lws *wsi, enum lws_callback_reasons reason, void *user, void *in, size_t len) {
  struct pss_http *pss = (struct pss_http *)user;
  unsigned char buffer[4096 + LWS_PRE], *p, *end;
  char buf[256];
  bool done = false;

  switch (reason) {
    case LWS_CALLBACK_HTTP:
      access_log(wsi, (const char *)in);
      snprintf(pss->path, sizeof(pss->path), "%s", (const char *)in);
      p = buffer + LWS_PRE;
      end = p + sizeof(buffer) - LWS_PRE;

      if (strcmp(pss->path, endpoints.Bell) == 0) {
        const char *content_type = "audio/mpeg";
        char *output = (char *)malloc(beep_mp3_len + 1);
        size_t output_len = beep_mp3_len;
        if (lws_add_http_header_status(wsi, HTTP_STATUS_OK, &p, end) ||
            lws_add_http_header_by_token(wsi, WSI_TOKEN_HTTP_CONTENT_TYPE,
                                          (const unsigned char *)content_type, 10, &p, end))
          return 1;

        if (lws_add_http_header_content_length(wsi, (unsigned long)output_len, &p, end) ||
            lws_finalize_http_header(wsi, &p, end) ||
            lws_write(wsi, buffer + LWS_PRE, p - (buffer + LWS_PRE), LWS_WRITE_HTTP_HEADERS) < 0)
          return 1;

        memcpy(output, beep_mp3, beep_mp3_len);
        pss->buffer = pss->ptr = output;
        pss->len = output_len;
        lws_callback_on_writable(wsi);
        break;
      }

      if (strncmp(pss->path, endpoints.favicon, strlen(endpoints.favicon)) == 0) {
        const char *rel = pss->path + strlen(endpoints.favicon);
        if (strstr(rel, "..") == NULL && strchr(rel, '/') == NULL && rel[0] != '\0') {
          char file_path[512];
          snprintf(file_path, sizeof(file_path), "/tmp/ttyd-favicons/%s", rel);
          int n = lws_serve_http_file(wsi, file_path, "image/png", NULL, 0);
          if (n < 0 || (n > 0 && lws_http_transaction_completed(wsi))) return 1;
        } else {
          lws_return_http_status(wsi, HTTP_STATUS_FORBIDDEN, NULL);
          goto try_to_reuse;
        }
        break;
      }

      if (strcmp(pss->path, endpoints.content) == 0 ||
          strncmp(pss->path, endpoints.content, strlen(endpoints.content)) == 0) {
        // parse ?lines=N and/or ?blocks=N
        int lines = 0, blocks = 0;
        char *qs = strchr(pss->path, '?');
        if (qs) {
          char *pp;
          if ((pp = strstr(qs, "lines=")) != NULL) lines = atoi(pp + 6);
          if ((pp = strstr(qs, "blocks=")) != NULL) blocks = atoi(pp + 7);
        }
        if (lines <= 0 && blocks <= 0) lines = 100;
        if (lines > 5000) lines = 5000;
        if (blocks > 100) blocks = 100;

        // linearize ring buffer
        size_t rlen = server->pty_ring_len;
        char *rbuf = xmalloc(rlen + 1);
        if (rlen > 0) {
          size_t start = (server->pty_ring_head + PTY_RING_SIZE - rlen) % PTY_RING_SIZE;
          size_t first = PTY_RING_SIZE - start;
          if (first >= rlen) {
            memcpy(rbuf, server->pty_ring + start, rlen);
          } else {
            memcpy(rbuf, server->pty_ring + start, first);
            memcpy(rbuf + first, server->pty_ring, rlen - first);
          }
        }
        rbuf[rlen] = '\0';

        // strip ANSI/VT escape sequences
        char *clean = xmalloc(rlen + 1);
        size_t ci = 0;
        for (size_t i = 0; i < rlen; i++) {
          unsigned char c = (unsigned char)rbuf[i];
          if (c == 0x1b) {
            i++;
            if (i < rlen && rbuf[i] == '[') {
              i++;
              while (i < rlen && (rbuf[i] < 0x40 || rbuf[i] > 0x7e)) i++;
            }
            continue;
          }
          if (c == '\r') continue;
          if (c >= 0x20 || c == '\n' || c == '\t') clean[ci++] = (char)c;
        }
        clean[ci] = '\0';
        free(rbuf);

        char *output = NULL;
        size_t output_len = 0;

        if (blocks > 0) {
          static const char *prompts[] = {"$ ", "% ", "# ", "> ", NULL};
          char **block_starts = xmalloc(8192 * sizeof(char *));
          int nblocks = 0;
          char *pos = clean;
          while ((pos = strchr(pos, '\n')) != NULL) {
            pos++;
            for (int pi = 0; prompts[pi]; pi++) {
              if (strncmp(pos, prompts[pi], strlen(prompts[pi])) == 0) {
                if (nblocks < 8192) block_starts[nblocks++] = pos + strlen(prompts[pi]);
                break;
              }
            }
          }
          int first_block = nblocks > blocks ? nblocks - blocks : 0;
          size_t out_alloc = rlen + (size_t)nblocks * 16 + 64;
          output = xmalloc(out_alloc);
          output_len = 0;
          for (int bi = first_block; bi < nblocks; bi++) {
            char *cmd_start = block_starts[bi];
            char *cmd_end = strchr(cmd_start, '\n');
            size_t cmd_len = cmd_end ? (size_t)(cmd_end - cmd_start) : strlen(cmd_start);
            output_len += (size_t)snprintf(output + output_len, out_alloc - output_len,
                                   "[command]: %.*s\n[output]: ", (int)cmd_len, cmd_start);
            char *out_start = cmd_end ? cmd_end + 1 : cmd_start + cmd_len;
            char *out_end = (bi + 1 < nblocks) ? block_starts[bi + 1] : clean + ci;
            if (out_end > out_start && output_len < out_alloc) {
              size_t olen = (size_t)(out_end - out_start);
              if (output_len + olen > out_alloc) olen = out_alloc - output_len - 1;
              memcpy(output + output_len, out_start, olen);
              output_len += olen;
            }
          }
          free(block_starts);
        } else {
          char *line_ptrs[5001];
          int nlines = 0;
          line_ptrs[nlines++] = clean;
          for (size_t i = 0; i < ci; i++) {
            if (clean[i] == '\n' && nlines < 5000) line_ptrs[nlines++] = clean + i + 1;
          }
          int first_line = nlines > lines ? nlines - lines : 0;
          char *slice_start = line_ptrs[first_line];
          output_len = ci - (size_t)(slice_start - clean);
          output = xmalloc(output_len + 1);
          memcpy(output, slice_start, output_len);
          output[output_len] = '\0';
        }
        free(clean);

        if (lws_add_http_header_status(wsi, HTTP_STATUS_OK, &p, end) ||
            lws_add_http_header_by_token(wsi, WSI_TOKEN_HTTP_CONTENT_TYPE,
                                         (unsigned char *)"text/plain;charset=utf-8", 24, &p, end) ||
            lws_add_http_header_by_name(wsi, (unsigned char *)"Access-Control-Allow-Origin:",
                                        (unsigned char *)"*", 1, &p, end) ||
            lws_add_http_header_content_length(wsi, (unsigned long)output_len, &p, end) ||
            lws_finalize_http_header(wsi, &p, end) ||
            lws_write(wsi, buffer + LWS_PRE, p - (buffer + LWS_PRE), LWS_WRITE_HTTP_HEADERS) < 0) {
          free(output);
          return 1;
        }
        pss->buffer = pss->ptr = output;
        pss->len = output_len;
        lws_callback_on_writable(wsi);
        break;
      }

      if (strncmp(pss->path, endpoints.index, strlen(endpoints.index)) != 0) {
        lws_return_http_status(wsi, HTTP_STATUS_NOT_FOUND, NULL);
        goto try_to_reuse;
      }

      const char *content_type = "text/html";
      {
        // always serve decompressed so we can inject meta tags
        char *html = NULL;
        size_t html_len = 0;
        if (!uncompress_html(&html, &html_len)) return 1;

        // extract cwd and app from request path for meta injection
        char meta_cwd[512] = {0};
        char meta_app[512] = {0};
        char *dir_p = strstr(pss->path, "/dir");
        if (dir_p && dir_p[4] == '/') {
          const char *s = dir_p + 4;
          const char *q = strchr(s, '?');
          size_t n = q ? (size_t)(q - s) : strlen(s);
          if (n < sizeof(meta_cwd)) { memcpy(meta_cwd, s, n); url_decode(meta_cwd); }
        }
        char *app_p = strstr(pss->path, "app=");
        if (app_p) {
          const char *s = app_p + 4;
          const char *q = strchr(s, '&');
          size_t n = q ? (size_t)(q - s) : strlen(s);
          if (n < sizeof(meta_app)) { memcpy(meta_app, s, n); url_decode(meta_app); }
        }

        // build meta block
        char meta_block[1024];
        int meta_len = 0;
        if (meta_cwd[0] || meta_app[0]) {
          char desc[768];
          if (meta_app[0] && meta_cwd[0])
            snprintf(desc, sizeof(desc), "Terminal: %s \xe2\x80\x94 %s", meta_app, meta_cwd);
          else if (meta_app[0])
            snprintf(desc, sizeof(desc), "Terminal: %s", meta_app);
          else
            snprintf(desc, sizeof(desc), "Terminal: %s", meta_cwd);

          char ogtitle[512];
          if (meta_app[0])
            snprintf(ogtitle, sizeof(ogtitle), "ttyd \xe2\x80\x94 %s", meta_app);
          else
            snprintf(ogtitle, sizeof(ogtitle), "ttyd \xe2\x80\x94 %s", meta_cwd);

          meta_len = snprintf(meta_block, sizeof(meta_block),
            "<meta name=\"description\" content=\"%s\">"
            "<meta property=\"og:title\" content=\"%s\">"
            "<meta property=\"og:description\" content=\"%s\">",
            desc, ogtitle, desc);
        }

        // inject before </head>
        char *inject_buf = NULL;
        size_t inject_len = 0;
        const char *head_close = memmem(html, html_len, "</head>", 7);
        if (head_close && meta_len > 0) {
          inject_len = html_len + (size_t)meta_len;
          inject_buf = xmalloc(inject_len);
          size_t prefix = (size_t)(head_close - html);
          memcpy(inject_buf, html, prefix);
          memcpy(inject_buf + prefix, meta_block, (size_t)meta_len);
          memcpy(inject_buf + prefix + meta_len, head_close, html_len - prefix);
        }

        char *output = inject_buf ? inject_buf : html;
        size_t output_len = inject_buf ? inject_len : html_len;

        if (lws_add_http_header_status(wsi, HTTP_STATUS_OK, &p, end) ||
            lws_add_http_header_by_token(wsi, WSI_TOKEN_HTTP_CONTENT_TYPE, (const unsigned char *)content_type, 9, &p,
                                         end) ||
            lws_add_http_header_content_length(wsi, (unsigned long)output_len, &p, end) ||
            lws_finalize_http_header(wsi, &p, end) ||
            lws_write(wsi, buffer + LWS_PRE, p - (buffer + LWS_PRE), LWS_WRITE_HTTP_HEADERS) < 0) {
          free(inject_buf);
          return 1;
        }

        pss->buffer = pss->ptr = inject_buf ? inject_buf : html;
        pss->len = output_len;
        lws_callback_on_writable(wsi);
      }
      break;

    case LWS_CALLBACK_HTTP_WRITEABLE:
      if (!pss->buffer || pss->len == 0) {
        goto try_to_reuse;
      }

      do {
        int n = sizeof(buffer) - LWS_PRE;
        int m = lws_get_peer_write_allowance(wsi);
        if (m == 0) {
          lws_callback_on_writable(wsi);
          return 0;
        } else if (m != -1 && m < n) {
          n = m;
        }
        if (pss->ptr + n > pss->buffer + pss->len) {
          n = (int)(pss->len - (pss->ptr - pss->buffer));
          done = true;
        }
        memcpy(buffer + LWS_PRE, pss->ptr, n);
        pss->ptr += n;
        if (lws_write_http(wsi, buffer + LWS_PRE, (size_t)n) < n) {
          pss_buffer_free(pss);
          return -1;
        }
      } while (!lws_send_pipe_choked(wsi) && !done);

      if (!done && pss->ptr < pss->buffer + pss->len) {
        lws_callback_on_writable(wsi);
        break;
      }

      pss_buffer_free(pss);
      goto try_to_reuse;

    case LWS_CALLBACK_HTTP_FILE_COMPLETION:
      goto try_to_reuse;
    default:
      break;
  }

  return 0;

  /* if we're on HTTP1.1 or 2.0, will keep the idle connection alive */
try_to_reuse:
  if (lws_http_transaction_completed(wsi)) return -1;

  return 0;
}
