#include "terminal.h"

#include <stdlib.h>
#include <string.h>

#include <libwebsockets.h>

#include "server.h"
#include "utils.h"

static int screen_damage(VTermRect rect, void *user) {
  terminal_t *term = (terminal_t *)user;
  VTermScreen *screen = term->screen;

  for (int row = rect.start_row; row < rect.end_row; row++) {
    for (int col = rect.start_col; col < rect.end_col; col++) {
      if (term->dirty_count >= TERM_MAX_CELLS) return 1;

      VTermScreenCell cell;
      VTermPos pos = {.row = row, .col = col};
      vterm_screen_get_cell(screen, pos, &cell);

      /* 0xFFFFFFFF is libvterm's sentinel for the right-half of a wide cell.
       * The left cell (width=2) already tells the client to paint 2 columns. */
      if (cell.chars[0] == 0xFFFFFFFFu) continue;

      VTermColor fg = cell.fg;
      VTermColor bg = cell.bg;
      vterm_screen_convert_color_to_rgb(screen, &fg);
      vterm_screen_convert_color_to_rgb(screen, &bg);

      cell_entry_t *e = &term->dirty[term->dirty_count++];
      e->row       = (uint16_t)row;
      e->col       = (uint16_t)col;
      e->codepoint = cell.chars[0] ? cell.chars[0] : 0x20;
      e->fg_r = fg.rgb.red;   e->fg_g = fg.rgb.green;   e->fg_b = fg.rgb.blue;
      e->bg_r = bg.rgb.red;   e->bg_g = bg.rgb.green;   e->bg_b = bg.rgb.blue;
      e->attrs = (uint8_t)(
          (cell.attrs.bold      ? 0x01 : 0) |
          (cell.attrs.italic    ? 0x02 : 0) |
          (cell.attrs.underline ? 0x04 : 0) |
          (cell.attrs.blink     ? 0x08 : 0) |
          (cell.attrs.reverse   ? 0x10 : 0) |
          (cell.attrs.strike    ? 0x20 : 0));
      e->width = (uint8_t)(cell.width ? cell.width : 1);

      /* For wide cells, explicitly enqueue col+1 when it falls outside the damage
       * rect — libvterm may stop the rect at col N and never iterate col N+1. */
      if (cell.width == 2 && col + 1 >= rect.end_col && col + 1 < term->cols) {
        if (term->dirty_count < TERM_MAX_CELLS) {
          VTermScreenCell nc;
          VTermPos npos = {.row = row, .col = col + 1};
          vterm_screen_get_cell(screen, npos, &nc);
          if (nc.chars[0] != 0xFFFFFFFFu) {
            VTermColor nfg = nc.fg, nbg = nc.bg;
            vterm_screen_convert_color_to_rgb(screen, &nfg);
            vterm_screen_convert_color_to_rgb(screen, &nbg);
            cell_entry_t *ne = &term->dirty[term->dirty_count++];
            ne->row = (uint16_t)row; ne->col = (uint16_t)(col + 1);
            ne->codepoint = nc.chars[0] ? nc.chars[0] : 0x20;
            ne->fg_r = nfg.rgb.red; ne->fg_g = nfg.rgb.green; ne->fg_b = nfg.rgb.blue;
            ne->bg_r = nbg.rgb.red; ne->bg_g = nbg.rgb.green; ne->bg_b = nbg.rgb.blue;
            ne->attrs = 0; ne->width = 1;
          }
        }
      }
    }
  }
  return 1;
}

static int screen_movecursor(VTermPos pos, VTermPos oldpos, int visible, void *user) {
  (void)pos; (void)oldpos;
  terminal_t *term = (terminal_t *)user;
  term->cursor_dirty = true;
  term->cursor_visible = (visible != 0);
  return 1;
}

static int screen_moverect(VTermRect dest, VTermRect src, void *user) {
  (void)src;
  return screen_damage(dest, user);
}

static int screen_sb_pushline(int cols, const VTermScreenCell *cells, void *user) {
  terminal_t *term = (terminal_t *)user;

  /* Evict oldest ring entry if full */
  if (term->sb_ring_count == SB_RING_SIZE) {
    free(term->sb_ring[term->sb_ring_head]->cells);
    free(term->sb_ring[term->sb_ring_head]);
    term->sb_ring[term->sb_ring_head] = NULL;
    term->sb_ring_head = (term->sb_ring_head + 1) % SB_RING_SIZE;
    term->sb_ring_count--;
    /* If send cursor was pointing at the evicted slot, advance it */
    if (term->sb_send_count > 0) {
      term->sb_send_pos = (term->sb_send_pos + 1) % SB_RING_SIZE;
      /* sb_send_count stays same — we lost one unsent line */
    }
  }

  sb_line_t *line = xmalloc(sizeof(sb_line_t));
  line->cols = (uint16_t)cols;
  line->cells = xmalloc(sizeof(cell_entry_t) * cols);
  line->next = NULL;

  for (int c = 0; c < cols; c++) {
    VTermScreenCell cell = cells[c];
    VTermColor fg = cell.fg, bg = cell.bg;
    vterm_screen_convert_color_to_rgb(term->screen, &fg);
    vterm_screen_convert_color_to_rgb(term->screen, &bg);
    cell_entry_t *e = &line->cells[c];
    e->row = 0; e->col = (uint16_t)c;
    e->codepoint = (cell.chars[0] == 0 || cell.chars[0] == 0xFFFFFFFFu) ? 0x20 : cell.chars[0];
    e->fg_r = fg.rgb.red; e->fg_g = fg.rgb.green; e->fg_b = fg.rgb.blue;
    e->bg_r = bg.rgb.red; e->bg_g = bg.rgb.green; e->bg_b = bg.rgb.blue;
    e->attrs = (uint8_t)(
        (cell.attrs.bold      ? 0x01 : 0) |
        (cell.attrs.italic    ? 0x02 : 0) |
        (cell.attrs.underline ? 0x04 : 0) |
        (cell.attrs.blink     ? 0x08 : 0) |
        (cell.attrs.reverse   ? 0x10 : 0) |
        (cell.attrs.strike    ? 0x20 : 0));
    e->width = (uint8_t)(cell.width ? cell.width : 1);
  }

  int tail = (term->sb_ring_head + term->sb_ring_count) % SB_RING_SIZE;
  term->sb_ring[tail] = line;
  term->sb_ring_count++;
  term->sb_send_count++;
  return 1;
}

static int screen_settermprop(VTermProp prop, VTermValue *val, void *user) {
  terminal_t *term = (terminal_t *)user;
  if (prop == VTERM_PROP_MOUSE) {
    uint8_t new_mode = (uint8_t)val->number;
    if (new_mode != term->mouse_mode) {
      term->mouse_mode = new_mode;
      term->mouse_mode_changed = true;
    }
  }
  if (prop == VTERM_PROP_ALTSCREEN) {
    uint8_t new_v = val->boolean ? 1 : 0;
    if (new_v != term->altscreen_active) {
      term->altscreen_active  = new_v;
      term->altscreen_changed = true;
    }
  }
  if (prop == VTERM_PROP_CURSORBLINK) {
    bool new_v = val->boolean != 0;
    if (new_v != term->cursor_blink_enabled) {
      term->cursor_blink_enabled = new_v;
      term->cursor_blink_changed = true;
    }
  }
  if (prop == VTERM_PROP_CURSORVISIBLE) {
    bool new_v = val->boolean != 0;
    if (new_v != term->cursor_visible) {
      term->cursor_visible = new_v;
      term->cursor_dirty = true;
    }
  }
  if (prop == VTERM_PROP_TITLE) {
    free(term->pending_title);
    term->pending_title = strdup(val->string.str);
    term->title_changed = true;
  }
  return 1;
}

static VTermScreenCallbacks screen_cbs = {
    .damage      = screen_damage,
    .movecursor  = screen_movecursor,
    .moverect    = screen_moverect,
    .settermprop = screen_settermprop,
    .sb_pushline = screen_sb_pushline,
};

const unsigned char *terminal_take_sb_line(terminal_t *term, size_t *out_len) {
  if (term->sb_send_count == 0) return NULL;

  sb_line_t *line = term->sb_ring[term->sb_send_pos];
  term->sb_send_pos = (term->sb_send_pos + 1) % SB_RING_SIZE;
  term->sb_send_count--;

  size_t needed = LWS_PRE + 3 + (size_t)line->cols * CELL_SIZE;
  if (needed > term->sb_buf_cap) {
    term->sb_buf_cap = needed;
    term->sb_buf = xrealloc(term->sb_buf, term->sb_buf_cap);
  }

  unsigned char *p = term->sb_buf + LWS_PRE;
  p[0] = SB_PUSH;
  p[1] = (uint8_t)(line->cols & 0xff);
  p[2] = (uint8_t)((line->cols >> 8) & 0xff);
  unsigned char *q = p + 3;
  for (int i = 0; i < line->cols; i++) {
    cell_entry_t *e = &line->cells[i];
    q[0]  = (uint8_t)(e->row & 0xff);
    q[1]  = 0;
    q[2]  = (uint8_t)(e->col & 0xff);
    q[3]  = (uint8_t)((e->col >> 8) & 0xff);
    q[4]  = (uint8_t)(e->codepoint & 0xff);
    q[5]  = (uint8_t)((e->codepoint >> 8)  & 0xff);
    q[6]  = (uint8_t)((e->codepoint >> 16) & 0xff);
    q[7]  = (uint8_t)((e->codepoint >> 24) & 0xff);
    q[8]  = e->fg_r; q[9]  = e->fg_g; q[10] = e->fg_b;
    q[11] = e->bg_r; q[12] = e->bg_g; q[13] = e->bg_b;
    q[14] = e->attrs;
    q[15] = e->width;
    q += CELL_SIZE;
  }

  *out_len = (size_t)(q - p);
  return p;
}

char *terminal_take_title(terminal_t *term) {
  if (!term->title_changed) return NULL;
  term->title_changed = false;
  char *t = term->pending_title;
  term->pending_title = NULL;
  return t;
}

void terminal_replay_sb(terminal_t *term) {
  term->sb_send_pos   = term->sb_ring_head;
  term->sb_send_count = term->sb_ring_count;
}

bool terminal_take_mouse_mode_change(terminal_t *term, uint8_t *out_mode) {
  if (!term->mouse_mode_changed) return false;
  term->mouse_mode_changed = false;
  *out_mode = term->mouse_mode;
  return true;
}

bool terminal_take_altscreen_change(terminal_t *term, uint8_t *out) {
  if (!term->altscreen_changed) return false;
  term->altscreen_changed = false;
  *out = term->altscreen_active;
  return true;
}

bool terminal_take_cursor_blink_change(terminal_t *term, bool *out) {
  if (!term->cursor_blink_changed) return false;
  term->cursor_blink_changed = false;
  *out = term->cursor_blink_enabled;
  return true;
}

terminal_t *terminal_create(uint16_t rows, uint16_t cols, void *pss) {
  terminal_t *term = xmalloc(sizeof(terminal_t));
  memset(term, 0, sizeof(terminal_t));
  term->rows = rows;
  term->cols = cols;
  term->pss  = pss;
  term->cursor_visible = true;
  term->cursor_blink_enabled = true;

  term->vt = vterm_new(rows, cols);
  vterm_set_utf8(term->vt, 1);

  term->screen = vterm_obtain_screen(term->vt);
  vterm_screen_set_callbacks(term->screen, &screen_cbs, term);
  vterm_screen_enable_altscreen(term->screen, 1);

  /* Default fg/bg must match the client theme — libvterm uses these when a cell
   * carries the "default color" marker (most cells, anything without explicit SGR). */
  VTermState *state = vterm_obtain_state(term->vt);
  VTermColor def_fg, def_bg;
  vterm_color_rgb(&def_fg, 212, 212, 212);  /* theme.foreground #D4D4D4 */
  vterm_color_rgb(&def_bg, 30, 30, 30);     /* theme.background #1E1E1E */
  vterm_state_set_default_colors(state, &def_fg, &def_bg);

  vterm_screen_reset(term->screen, 1);

  term->frame_buf_cap = LWS_PRE + CELL_DIFF_HEADER_SIZE + TERM_MAX_CELLS * CELL_SIZE;
  term->frame_buf = xmalloc(term->frame_buf_cap);

  /* sb_buf: SB_PUSH(1) + cols(2) + cols * CELL_SIZE bytes; grows on demand */
  term->sb_buf_cap = LWS_PRE + 3 + (size_t)cols * CELL_SIZE;
  term->sb_buf = xmalloc(term->sb_buf_cap);

  return term;
}

void terminal_clear(terminal_t *term) {
  for (int i = 0; i < term->sb_ring_count; i++) {
    int idx = (term->sb_ring_head + i) % SB_RING_SIZE;
    free(term->sb_ring[idx]->cells);
    free(term->sb_ring[idx]);
    term->sb_ring[idx] = NULL;
  }
  term->sb_ring_head = term->sb_ring_count = term->sb_send_pos = term->sb_send_count = 0;
  /* Clear libvterm's active screen directly so resize won't replay old content. */
  const char seq[] = "\x1b[H\x1b[2J";
  vterm_input_write(term->vt, seq, sizeof(seq) - 1);
  vterm_screen_flush_damage(term->screen);
}

void terminal_destroy(terminal_t *term) {
  if (term == NULL) return;
  if (term->vt) vterm_free(term->vt);
  free(term->frame_buf);
  free(term->sb_buf);
  free(term->pending_title);
  for (int i = 0; i < term->sb_ring_count; i++) {
    int idx = (term->sb_ring_head + i) % SB_RING_SIZE;
    free(term->sb_ring[idx]->cells);
    free(term->sb_ring[idx]);
  }
  free(term);
}

bool terminal_push(terminal_t *term, const char *data, size_t len) {
  int before = term->dirty_count;
  bool cursor_before = term->cursor_dirty;
  vterm_input_write(term->vt, data, len);
  vterm_screen_flush_damage(term->screen);
  return term->dirty_count > before || (term->cursor_dirty && !cursor_before);
}

void terminal_resize(terminal_t *term, uint16_t rows, uint16_t cols) {
  term->rows = rows;
  term->cols = cols;
  vterm_set_size(term->vt, rows, cols);
  vterm_screen_flush_damage(term->screen);
}

const unsigned char *terminal_encode_frame(terminal_t *term, size_t *out_len) {
  unsigned char *p = term->frame_buf + LWS_PRE;
  int n = term->dirty_count;

  VTermState *state = vterm_obtain_state(term->vt);
  VTermPos cursor = {0, 0};
  vterm_state_get_cursorpos(state, &cursor);

  /* 8-byte header */
  p[0] = CELL_DIFF;
  p[1] = term->cursor_visible ? 0x01 : 0x00;  /* flags: cursor_visible */
  p[2] = (uint8_t)(cursor.row & 0xff);
  p[3] = (uint8_t)((cursor.row >> 8) & 0xff);
  p[4] = (uint8_t)(cursor.col & 0xff);
  p[5] = (uint8_t)((cursor.col >> 8) & 0xff);
  p[6] = (uint8_t)(n & 0xff);
  p[7] = (uint8_t)((n >> 8) & 0xff);

  unsigned char *q = p + CELL_DIFF_HEADER_SIZE;
  for (int i = 0; i < n; i++) {
    cell_entry_t *e = &term->dirty[i];
    q[0]  = (uint8_t)(e->row & 0xff);
    q[1]  = (uint8_t)((e->row >> 8) & 0xff);
    q[2]  = (uint8_t)(e->col & 0xff);
    q[3]  = (uint8_t)((e->col >> 8) & 0xff);
    q[4]  = (uint8_t)(e->codepoint & 0xff);
    q[5]  = (uint8_t)((e->codepoint >> 8)  & 0xff);
    q[6]  = (uint8_t)((e->codepoint >> 16) & 0xff);
    q[7]  = (uint8_t)((e->codepoint >> 24) & 0xff);
    q[8]  = e->fg_r; q[9]  = e->fg_g; q[10] = e->fg_b;
    q[11] = e->bg_r; q[12] = e->bg_g; q[13] = e->bg_b;
    q[14] = e->attrs;
    q[15] = e->width;
    q += CELL_SIZE;
  }

  *out_len = (size_t)(q - p);
  /* clear dirty list now that it's been consumed */
  term->dirty_count  = 0;
  term->cursor_dirty = false;
  return p;
}

void terminal_mark_all_dirty(terminal_t *term) {
  VTermRect all = {.start_row = 0, .end_row = term->rows,
                   .start_col = 0, .end_col = term->cols};
  term->dirty_count  = 0;
  term->cursor_dirty = true;
  screen_damage(all, term);
}
