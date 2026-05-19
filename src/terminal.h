#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include <vterm.h>

#define TERM_MAX_CELLS        16384
#define CELL_DIFF_HEADER_SIZE 8       /* type(1)+flags(1)+cur_row(2)+cur_col(2)+count(2) */
#define CELL_SIZE             16      /* row(2)+col(2)+cp(4)+fg(3)+bg(3)+attrs(1)+width(1) */
#define SB_RING_SIZE          2000    /* persistent scrollback ring; replayed on reattach */

typedef struct {
  uint16_t row, col;
  uint32_t codepoint;
  uint8_t  fg_r, fg_g, fg_b;
  uint8_t  bg_r, bg_g, bg_b;
  uint8_t  attrs;
  uint8_t  width;
} cell_entry_t;

typedef struct sb_line_s {
  uint16_t cols;
  cell_entry_t *cells;
  struct sb_line_s *next;
} sb_line_t;

typedef struct terminal_s {
  VTerm       *vt;
  VTermScreen *screen;
  uint16_t     rows, cols;

  cell_entry_t dirty[TERM_MAX_CELLS];
  int          dirty_count;
  bool         cursor_dirty;
  bool         cursor_visible;
  bool         cursor_blink_enabled;
  bool         cursor_blink_changed;

  unsigned char *frame_buf;
  size_t         frame_buf_cap;

  uint8_t mouse_mode;          /* libvterm mouse mode: 0/1/2/3 */
  bool    mouse_mode_changed;

  uint8_t altscreen_active;    /* 0=primary, 1=alt */
  bool    altscreen_changed;

  char   *pending_title;       /* heap string, NULL if unchanged */
  bool    title_changed;

  /* persistent scrollback ring — lines stay until evicted by newer ones */
  sb_line_t *sb_ring[SB_RING_SIZE];
  int        sb_ring_head;     /* index of oldest entry */
  int        sb_ring_count;    /* number of valid entries */

  /* per-client send cursor into the ring */
  int        sb_send_pos;      /* ring index of next line to send */
  int        sb_send_count;    /* how many lines remain to send */

  unsigned char *sb_buf;       /* encoding scratch buffer */
  size_t         sb_buf_cap;

  void *pss;
} terminal_t;

terminal_t          *terminal_create(uint16_t rows, uint16_t cols, void *pss);
void                 terminal_destroy(terminal_t *term);
bool                 terminal_push(terminal_t *term, const char *data, size_t len);
void                 terminal_resize(terminal_t *term, uint16_t rows, uint16_t cols);
const unsigned char *terminal_encode_frame(terminal_t *term, size_t *out_len);
void                 terminal_mark_all_dirty(terminal_t *term);
bool                 terminal_take_mouse_mode_change(terminal_t *term, uint8_t *out_mode);
bool                 terminal_take_altscreen_change(terminal_t *term, uint8_t *out);
bool                 terminal_take_cursor_blink_change(terminal_t *term, bool *out);
char                *terminal_take_title(terminal_t *term);
/* Encode + dequeue one pending scrollback line for sending to current client.
 * The line stays in the ring for replay on reattach.
 * Pointer is valid until next call; do not free.  */
const unsigned char *terminal_take_sb_line(terminal_t *term, size_t *out_len);
/* Reset send cursor so the full ring is replayed to a newly attached client. */
void                 terminal_replay_sb(terminal_t *term);
/* Clear and free all scrollback history, and blank the libvterm screen. */
void                 terminal_clear(terminal_t *term);
