#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include <vterm.h>

#define TERM_MAX_CELLS        16384
#define CELL_DIFF_HEADER_SIZE 8       /* type(1)+flags(1)+cur_row(2)+cur_col(2)+count(2) */
#define CELL_SIZE             16      /* row(2)+col(2)+cp(4)+fg(3)+bg(3)+attrs(1)+width(1) */
#define SB_MAX_QUEUED         200     /* cap on lines waiting to be sent in one batch */

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

  unsigned char *frame_buf;
  size_t         frame_buf_cap;

  uint8_t mouse_mode;          /* libvterm mouse mode: 0/1/2/3 */
  bool    mouse_mode_changed;

  /* scrollback queue: lines that have been pushed off the top */
  sb_line_t *sb_head;
  sb_line_t *sb_tail;
  int        sb_pending;
  unsigned char *sb_buf;
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
/* Encode + dequeue one pending scrollback line. Returns NULL if none queued.
 * Pointer is valid until next call; do not free.  */
const unsigned char *terminal_take_sb_line(terminal_t *term, size_t *out_len);
