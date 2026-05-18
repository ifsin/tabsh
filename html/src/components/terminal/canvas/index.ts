// Canvas-based terminal client. The server (libvterm) does all VT parsing;
// this file just paints cell-diff frames and ships keystrokes back.
import { bind } from 'decko';

// Commands server → client
const enum S2C {
    OUTPUT_LEGACY = '0',
    SET_WINDOW_TITLE = '1',
    SET_PREFERENCES = '2',
    SET_APP_COMMAND = '3',
    SET_REATTACHED = '4',
    SET_APP_FAVICON = '5',
    CELL_DIFF = '6',
    SB_PUSH = '7',
    MOUSE_MODE = '8',
    ALT_SCREEN = '9',
}

// Commands client → server
const enum C2S {
    INPUT = '0',
    RESIZE_TERMINAL = '1',
    PAUSE = '2',
    RESUME = '3',
    QUIT = '4',
}

const ATTR_BOLD = 0x01;
const ATTR_ITALIC = 0x02;
const ATTR_UNDERLINE = 0x04;
const ATTR_REVERSE = 0x10;

interface Cell {
    cp: number;
    fr: number; fg: number; fb: number;
    br: number; bg: number; bb: number;
    attrs: number;
    width: number;
}

export interface ClientOptions {
    rendererType?: string;
    disableLeaveAlert?: boolean;
    disableResizeOverlay?: boolean;
    enableZmodem?: boolean;
    enableTrzsz?: boolean;
    enableSixel?: boolean;
    titleFixed?: string;
    isWindows?: boolean;
    homeDir?: string;
    trzszDragInitTimeout?: number;
    unicodeVersion?: string;
    closeOnDisconnect?: boolean;
}

export interface FlowControl {
    limit: number;
    highWater: number;
    lowWater: number;
}

export interface XtermOptions {
    wsUrl: string;
    tokenUrl: string;
    flowControl: FlowControl;
    clientOptions: ClientOptions;
    termOptions: {
        fontSize?: number;
        fontFamily?: string;
        theme?: { foreground?: string; background?: string; cursor?: string };
    };
}

function blankCell(fg: [number, number, number], bg: [number, number, number]): Cell {
    return { cp: 0x20, fr: fg[0], fg: fg[1], fb: fg[2], br: bg[0], bg: bg[1], bb: bg[2], attrs: 0, width: 1 };
}

function rowToText(row: Cell[]): string {
    let s = '';
    for (const c of row) s += String.fromCodePoint(c.cp || 0x20);
    return s.replace(/\s+$/, '');
}

function parseHexColor(hex?: string): [number, number, number] {
    if (!hex || hex[0] !== '#' || hex.length !== 7) return [223, 219, 221];
    return [parseInt(hex.slice(1, 3), 16), parseInt(hex.slice(3, 5), 16), parseInt(hex.slice(5, 7), 16)];
}

const SB_MAX_LINES = 10000;

class CanvasRenderer {
    canvas: HTMLCanvasElement;
    scrollWrap: HTMLDivElement;
    private scrollInner: HTMLDivElement;
    private ctx: CanvasRenderingContext2D;
    private grid: Cell[][] = [];
    rows = 24;
    cols = 80;
    cellW = 8;
    cellH = 16;
    private fontSize: number;
    private fontFamily: string;
    private dpr: number;
    private cursorRow = 0;
    private cursorCol = 0;
    private cursorVisible = true;
    private fgDefault: [number, number, number];
    private bgDefault: [number, number, number];
    private cursorColor: [number, number, number];
    private fontNormal = '';
    private fontBold = '';
    private fontItalic = '';
    private fontBoldItalic = '';
    scrollback: Cell[][] = [];
    // droppedCount = total lines ever evicted from sb head; keeps absLine stable.
    private droppedCount = 0;
    // viewFirstLine = absolute line index at the top of the canvas viewport.
    viewFirstLine = 0;
    private altScreenActive = false;
    sel?: { aLine: number; aCol: number; fLine: number; fCol: number };

    constructor(parent: HTMLElement, fontSize: number, fontFamily: string, theme: { foreground?: string; background?: string; cursor?: string }) {
        this.fontSize = fontSize;
        this.fontFamily = fontFamily;
        this.fgDefault = parseHexColor(theme.foreground);
        this.bgDefault = parseHexColor(theme.background);
        this.cursorColor = parseHexColor(theme.cursor);
        this.dpr = window.devicePixelRatio || 1;

        this.scrollWrap = document.createElement('div');
        this.scrollWrap.className = 'scroll-wrap';

        this.scrollInner = document.createElement('div');
        this.scrollInner.className = 'scroll-inner';

        this.canvas = document.createElement('canvas');
        this.canvas.tabIndex = 0;
        this.canvas.style.cssText = 'display:block;outline:none;position:sticky;top:0';
        this.canvas.style.backgroundColor = `rgb(${this.bgDefault.join(',')})`;
        parent.style.backgroundColor = `rgb(${this.bgDefault.join(',')})`;

        this.scrollInner.appendChild(this.canvas);
        this.scrollWrap.appendChild(this.scrollInner);
        parent.appendChild(this.scrollWrap);

        this.ctx = this.canvas.getContext('2d')!;
        this.measureCell();

        this.scrollWrap.addEventListener('scroll', () => this.onScroll(), { passive: true });
    }

    private measureCell() {
        this.fontNormal = `${this.fontSize}px ${this.fontFamily}`;
        this.fontBold = `bold ${this.fontSize}px ${this.fontFamily}`;
        this.fontItalic = `italic ${this.fontSize}px ${this.fontFamily}`;
        this.fontBoldItalic = `italic bold ${this.fontSize}px ${this.fontFamily}`;

        this.ctx.font = this.fontNormal;
        const m = this.ctx.measureText('M');
        this.cellW = Math.round(Math.max(1, Math.round(m.width)) * this.dpr) / this.dpr;
        this.cellH = Math.round(Math.max(1, Math.round(this.fontSize * 1.3)) * this.dpr) / this.dpr;
    }

    focus() { this.canvas.focus(); }

    fit(width: number, height: number): { cols: number; rows: number } {
        const cols = Math.max(1, Math.floor(width / this.cellW));
        const rows = Math.max(1, Math.floor(height / this.cellH));
        if (cols === this.cols && rows === this.rows && this.canvas.width > 0) {
            return { cols, rows };
        }
        this.cols = cols;
        this.rows = rows;

        const newGrid: Cell[][] = [];
        for (let r = 0; r < rows; r++) {
            const row: Cell[] = [];
            for (let c = 0; c < cols; c++) {
                row.push(this.grid[r]?.[c] ?? blankCell(this.fgDefault, this.bgDefault));
            }
            newGrid.push(row);
        }
        this.grid = newGrid;

        const w = cols * this.cellW;
        const h = rows * this.cellH;
        this.canvas.style.width = `${w}px`;
        this.canvas.style.height = `${h}px`;
        this.canvas.width = Math.round(w * this.dpr);
        this.canvas.height = Math.round(h * this.dpr);
        this.ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
        this.ctx.textBaseline = 'top';

        this.updateInnerHeight();
        this.viewFirstLine = this.droppedCount + Math.floor(this.scrollWrap.scrollTop / this.cellH);
        this.repaintViewport();
        return { cols, rows };
    }

    private updateInnerHeight() {
        if (this.altScreenActive) {
            this.scrollInner.style.height = `${this.rows * this.cellH}px`;
        } else {
            this.scrollInner.style.height = `${(this.scrollback.length + this.rows) * this.cellH}px`;
        }
    }

    // scrollTop = sb.length * cellH means grid[0] is at the top → we're at the live screen.
    private isAtBottom(): boolean {
        const target = this.scrollback.length * this.cellH;
        return this.scrollWrap.scrollTop >= target - this.cellH;
    }

    private setScrollTop(top: number) {
        this.scrollWrap.scrollTop = top;
        // Programmatic scrollTop changes don't fire scroll events; sync manually.
        this.viewFirstLine = this.droppedCount + Math.floor(top / this.cellH);
    }

    private snapToBottom() {
        this.setScrollTop(this.scrollback.length * this.cellH);
    }

    private onScroll() {
        const firstLine = this.droppedCount + Math.floor(this.scrollWrap.scrollTop / this.cellH);
        if (firstLine !== this.viewFirstLine) {
            this.viewFirstLine = firstLine;
            this.repaintViewport();
        }
    }

    repaintViewport() {
        this.ctx.fillStyle = `rgb(${this.bgDefault.join(',')})`;
        this.ctx.fillRect(0, 0, this.cols * this.cellW, this.rows * this.cellH);

        const sbLen = this.scrollback.length;
        for (let viewRow = 0; viewRow < this.rows; viewRow++) {
            const absLine = this.viewFirstLine + viewRow;
            const sbIdx = absLine - this.droppedCount;
            for (let col = 0; col < this.cols; col++) {
                let cell: Cell;
                if (sbIdx < 0) {
                    cell = blankCell(this.fgDefault, this.bgDefault);
                } else if (sbIdx < sbLen) {
                    cell = this.scrollback[sbIdx]?.[col] ?? blankCell(this.fgDefault, this.bgDefault);
                } else {
                    cell = this.grid[sbIdx - sbLen]?.[col] ?? blankCell(this.fgDefault, this.bgDefault);
                }
                this.paintCellAt(viewRow, col, cell, false);
            }
        }

        // Draw cursor only when it falls within the visible viewport.
        const cursorAbsLine = this.droppedCount + sbLen + this.cursorRow;
        if (this.cursorVisible &&
            cursorAbsLine >= this.viewFirstLine &&
            cursorAbsLine < this.viewFirstLine + this.rows &&
            this.cursorRow < this.rows && this.cursorCol < this.cols) {
            const cursorViewRow = cursorAbsLine - this.viewFirstLine;
            const cell = this.grid[this.cursorRow]?.[this.cursorCol];
            if (cell) this.paintCellAtXY(this.cursorCol * this.cellW, cursorViewRow * this.cellH, cell, true);
        }

        this.paintSelectionOverlay();
    }

    private paintSelectionOverlay() {
        if (!this.sel) return;
        const { aLine, aCol, fLine, fCol } = this.sel;
        const startLine = Math.min(aLine, fLine);
        const endLine = Math.max(aLine, fLine);
        const startCol = aLine <= fLine ? aCol : fCol;
        const endCol = aLine <= fLine ? fCol : aCol;

        this.ctx.fillStyle = 'rgba(80, 140, 220, 0.35)';
        const visStart = Math.max(startLine, this.viewFirstLine);
        const visEnd = Math.min(endLine, this.viewFirstLine + this.rows - 1);
        for (let absLine = visStart; absLine <= visEnd; absLine++) {
            const viewRow = absLine - this.viewFirstLine;
            let sc: number, ec: number;
            if (startLine === endLine) {
                sc = Math.min(startCol, endCol);
                ec = Math.max(startCol, endCol) + 1;
            } else if (absLine === startLine) {
                sc = startCol; ec = this.cols;
            } else if (absLine === endLine) {
                sc = 0; ec = endCol + 1;
            } else {
                sc = 0; ec = this.cols;
            }
            this.ctx.fillRect(sc * this.cellW, viewRow * this.cellH, (ec - sc) * this.cellW, this.cellH);
        }
    }

    buildSelectionText(): string {
        if (!this.sel) return '';
        const { aLine, aCol, fLine, fCol } = this.sel;
        const startLine = Math.min(aLine, fLine);
        const endLine = Math.max(aLine, fLine);
        const startCol = aLine <= fLine ? aCol : fCol;
        const endCol = aLine <= fLine ? fCol : aCol;

        const lines: string[] = [];
        for (let absLine = startLine; absLine <= endLine; absLine++) {
            const sbIdx = absLine - this.droppedCount;
            let row: Cell[];
            if (sbIdx < 0) { lines.push(''); continue; }
            if (sbIdx < this.scrollback.length) {
                row = this.scrollback[sbIdx] ?? [];
            } else {
                row = this.grid[sbIdx - this.scrollback.length] ?? [];
            }
            let sc = 0, ec = row.length - 1;
            if (startLine === endLine) {
                sc = Math.min(startCol, endCol); ec = Math.max(startCol, endCol);
            } else if (absLine === startLine) {
                sc = startCol; ec = row.length - 1;
            } else if (absLine === endLine) {
                ec = endCol;
            }
            let s = '';
            for (let c = sc; c <= ec && c < row.length; c++) {
                s += String.fromCodePoint(row[c]?.cp || 0x20);
            }
            lines.push(s.replace(/\s+$/, ''));
        }
        return lines.join('\n');
    }

    pushScrollback(line: Cell[]) {
        const wasAtBottom = this.isAtBottom();
        this.scrollback.push(line);
        if (this.scrollback.length > SB_MAX_LINES) {
            const excess = this.scrollback.length - SB_MAX_LINES;
            this.scrollback.splice(0, excess);
            this.droppedCount += excess;
            if (this.sel && Math.min(this.sel.aLine, this.sel.fLine) < this.droppedCount) {
                this.sel = undefined;
            }
            if (!wasAtBottom) {
                // Adjust scrollTop so the view stays on the same content after eviction.
                const newTop = Math.max(0, this.scrollWrap.scrollTop - excess * this.cellH);
                this.setScrollTop(newTop);
            }
        }
        this.updateInnerHeight();
        if (wasAtBottom) this.snapToBottom();
    }

    applyFrame(buf: ArrayBuffer) {
        const v = new DataView(buf);
        const flags = v.getUint8(0);
        const curRow = v.getUint16(1, true);
        const curCol = v.getUint16(3, true);
        const count = v.getUint16(5, true);
        this.cursorVisible = (flags & 0x01) !== 0;

        const wasAtBottom = this.isAtBottom();

        // Pre-scan: clear selection if any incoming cell falls within it.
        if (this.sel) {
            const sbLen = this.scrollback.length;
            const selMinLine = Math.min(this.sel.aLine, this.sel.fLine);
            const selMaxLine = Math.max(this.sel.aLine, this.sel.fLine);
            let scanO = 7;
            for (let i = 0; i < count && this.sel; i++) {
                const row = v.getUint16(scanO, true);
                const absLine = this.droppedCount + sbLen + row;
                if (absLine >= selMinLine && absLine <= selMaxLine) {
                    this.sel = undefined;
                }
                scanO += 16; // CELL_SIZE
            }
        }

        let o = 7;
        for (let i = 0; i < count; i++) {
            const row = v.getUint16(o, true); o += 2;
            const col = v.getUint16(o, true); o += 2;
            const cp = v.getUint32(o, true); o += 4;
            const fr = v.getUint8(o++); const fg = v.getUint8(o++); const fb = v.getUint8(o++);
            const br = v.getUint8(o++); const bg = v.getUint8(o++); const bb = v.getUint8(o++);
            const attrs = v.getUint8(o++); const width = v.getUint8(o++);

            if (row >= this.rows || col >= this.cols) continue;
            const prev = this.grid[row][col];
            const cell: Cell = { cp, fr, fg, fb, br, bg, bb, attrs, width };
            this.grid[row][col] = cell;
            if (width >= 2 && col + 1 < this.cols) {
                this.grid[row][col + 1] = { cp: 0x20, fr, fg, fb, br, bg, bb, attrs: 0, width: 1 };
            }
            // If old cell was wide, clear col+1 so it doesn't ghost.
            if (prev && prev.width >= 2 && col + 1 < this.cols) {
                const cleared: Cell = { cp: 0x20, fr: cell.fr, fg: cell.fg, fb: cell.fb,
                                        br: cell.br, bg: cell.bg, bb: cell.bb, attrs: 0, width: 1 };
                this.grid[row][col + 1] = cleared;
            }
        }

        this.cursorRow = curRow;
        this.cursorCol = curCol;
        if (wasAtBottom) this.snapToBottom();
        this.repaintViewport();
    }

    applySbPush(buf: ArrayBuffer) {
        const v = new DataView(buf);
        const cols = v.getUint16(0, true);
        const line: Cell[] = [];
        let o = 2;
        for (let i = 0; i < cols; i++) {
            o += 4; // skip row(2) + col(2) — not needed for scrollback display
            const cp = v.getUint32(o, true); o += 4;
            const fr = v.getUint8(o++); const fg = v.getUint8(o++); const fb = v.getUint8(o++);
            const br = v.getUint8(o++); const bg = v.getUint8(o++); const bb = v.getUint8(o++);
            const attrs = v.getUint8(o++); const width = v.getUint8(o++);
            line.push({ cp, fr, fg, fb, br, bg, bb, attrs, width });
        }
        this.pushScrollback(line);
    }

    setAltScreen(on: boolean) {
        this.altScreenActive = on;
        if (on) {
            this.scrollWrap.style.overflowY = 'hidden';
            this.scrollInner.style.height = `${this.rows * this.cellH}px`;
            this.scrollWrap.scrollTop = 0;
            this.viewFirstLine = this.droppedCount + this.scrollback.length;
            this.sel = undefined;
        } else {
            this.scrollWrap.style.overflowY = '';
            this.updateInnerHeight();
            this.snapToBottom();
        }
        this.repaintViewport();
    }

    get isAltScreen() { return this.altScreenActive; }

    private paintCellAt(viewRow: number, col: number, cell: Cell, isCursor: boolean) {
        this.paintCellAtXY(col * this.cellW, viewRow * this.cellH, cell, isCursor);
    }

    private paintCellAtXY(x: number, y: number, cell: Cell, isCursor: boolean) {
        const reverse = (cell.attrs & ATTR_REVERSE) !== 0;
        let fr = cell.fr, fg = cell.fg, fb = cell.fb;
        let br = cell.br, bg = cell.bg, bb = cell.bb;
        if (reverse) { [fr, fg, fb, br, bg, bb] = [br, bg, bb, fr, fg, fb]; }
        if (isCursor) {
            br = this.cursorColor[0]; bg = this.cursorColor[1]; bb = this.cursorColor[2];
            fr = this.bgDefault[0]; fg = this.bgDefault[1]; fb = this.bgDefault[2];
        }

        const wide = Math.max(1, cell.width || 1) > 1;
        const cellSpan = wide ? this.cellW * 2 : this.cellW;

        this.ctx.fillStyle = `rgb(${br},${bg},${bb})`;
        this.ctx.fillRect(x, y, cellSpan, this.cellH);

        if (cell.cp >= 0x20 && cell.cp !== 0xa0) {
            const bold = (cell.attrs & ATTR_BOLD) !== 0;
            const italic = (cell.attrs & ATTR_ITALIC) !== 0;
            this.ctx.font = bold && italic ? this.fontBoldItalic
                : bold ? this.fontBold
                    : italic ? this.fontItalic
                        : this.fontNormal;
            this.ctx.fillStyle = `rgb(${fr},${fg},${fb})`;

            if (wide) {
                // Clip wide glyphs to their span so they can't bleed into unrelated cells.
                this.ctx.save();
                this.ctx.beginPath();
                this.ctx.rect(x, y, cellSpan, this.cellH);
                this.ctx.clip();
                this.ctx.fillText(String.fromCodePoint(cell.cp), x, y + 1);
                this.ctx.restore();
                this.ctx.textBaseline = 'top'; // restore after save/restore
            } else {
                this.ctx.fillText(String.fromCodePoint(cell.cp), x, y + 1);
            }
        }

        if (cell.attrs & ATTR_UNDERLINE) {
            this.ctx.fillStyle = `rgb(${fr},${fg},${fb})`;
            this.ctx.fillRect(x, y + this.cellH - 2, cellSpan, 1);
        }
    }
}

function encodeKey(e: KeyboardEvent): string | null {
    const k = e.key;

    if (e.ctrlKey && !e.metaKey && !e.altKey) {
        if (k.length === 1) {
            const c = k.toLowerCase().charCodeAt(0);
            if (c >= 0x40 && c <= 0x7f) return String.fromCharCode(c - 0x60 & 0x1f);
            if (k === ' ') return '\x00';
        }
    }

    // Alt+key → ESC prefix (readline Meta sequences: M-f, M-b, etc.)
    if (e.altKey && !e.ctrlKey && !e.metaKey && k.length === 1) {
        return '\x1b' + k;
    }

    switch (k) {
        case 'Enter': return e.shiftKey ? '\n' : '\r';
        case 'Backspace': return '\x7f';
        case 'Tab': return e.shiftKey ? '\x1b[Z' : '\t';
        case 'Escape': return '\x1b';
        case 'ArrowUp': return '\x1b[A';
        case 'ArrowDown': return '\x1b[B';
        case 'ArrowRight': return '\x1b[C';
        case 'ArrowLeft': return '\x1b[D';
        case 'Home': return '\x1b[H';
        case 'End': return '\x1b[F';
        case 'PageUp': return '\x1b[5~';
        case 'PageDown': return '\x1b[6~';
        case 'Delete': return '\x1b[3~';
        case 'Insert': return '\x1b[2~';
        case 'F1': return '\x1bOP';
        case 'F2': return '\x1bOQ';
        case 'F3': return '\x1bOR';
        case 'F4': return '\x1bOS';
        case 'F5': return '\x1b[15~';
        case 'F6': return '\x1b[17~';
        case 'F7': return '\x1b[18~';
        case 'F8': return '\x1b[19~';
        case 'F9': return '\x1b[20~';
        case 'F10': return '\x1b[21~';
        case 'F11': return '\x1b[23~';
        case 'F12': return '\x1b[24~';
    }
    if (k.length === 1 && !e.ctrlKey && !e.metaKey) {
        return k;
    }
    return null;
}

export class Xterm {
    private textEncoder = new TextEncoder();
    private textDecoder = new TextDecoder();
    private socket?: WebSocket;
    private token: string = '';
    private sessionId: string;
    private renderer?: CanvasRenderer;
    private parent?: HTMLElement;
    private resizeObs?: ResizeObserver;
    private audio?: HTMLAudioElement;
    private reconnect = true;
    private doReconnect = true;
    private closeOnDisconnect = false;
    private opened = false;
    private titleFixed?: string;
    private currentTitle?: string;
    private resizeTimer = 0;
    private mouseMode = 0;
    private mouseDownBtn = -1;

    private static generateSessionId(): string {
        const key = 'ttyd-session-id';
        const stored = sessionStorage.getItem(key);
        if (stored) return stored;
        const id = crypto.randomUUID();
        sessionStorage.setItem(key, id);
        return id;
    }

    constructor(private options: XtermOptions, private sendCb?: () => void) {
        this.sessionId = Xterm.generateSessionId();
    }

    dispose() {
        try { this.socket?.close(); } catch { /* ignore */ }
        this.resizeObs?.disconnect();
        window.removeEventListener('beforeunload', this.onBeforeUnload);
    }

    @bind
    public async refreshToken() {
        try {
            const resp = await fetch(this.options.tokenUrl);
            if (resp.ok) {
                const json = await resp.json();
                this.token = json.token ?? '';
            }
        } catch (e) {
            console.error(`[tabsh] fetch ${this.options.tokenUrl}: `, e);
        }
    }

    @bind
    public sendFile(_files: FileList) {
        console.warn('[tabsh] file transfer disabled in cell-diff mode');
    }

    @bind
    public open(parent: HTMLElement) {
        this.parent = parent;
        parent.style.width = '100%';
        parent.style.height = '100vh';
        const fontSize = this.options.termOptions.fontSize ?? 13;
        const fontFamily = this.options.termOptions.fontFamily ?? 'monospace';
        this.renderer = new CanvasRenderer(parent, fontSize, fontFamily, this.options.termOptions.theme ?? {});
        this.renderer.fit(parent.clientWidth || window.innerWidth, parent.clientHeight || window.innerHeight);
        this.installA11yMirror();

        this.resizeObs = new ResizeObserver(() => this.onResize());
        this.resizeObs.observe(parent);
        window.addEventListener('resize', () => this.onResize());
        window.addEventListener('beforeunload', this.onBeforeUnload);
        this.attachInput();
    }

    private a11yEl?: HTMLPreElement;
    private a11yTimer = 0;

    private installA11yMirror() {
        const pre = document.createElement('pre');
        pre.id = 'tabsh-buffer';
        pre.setAttribute('aria-label', 'terminal contents');
        pre.setAttribute('aria-live', 'polite');
        pre.setAttribute('role', 'log');
        pre.style.cssText = 'position:absolute;left:-9999px;top:0;width:1px;height:1px;overflow:hidden;white-space:pre;font-family:monospace';
        document.body.appendChild(pre);
        this.a11yEl = pre;

        const hint = document.createElement('div');
        hint.id = 'tabsh-api';
        hint.style.cssText = pre.style.cssText;
        hint.textContent =
            'Terminal content available via:\n' +
            '- DOM: #tabsh-buffer (scrollback + current view)\n' +
            '- HTTP: GET /content?lines=N or ?blocks=N';
        document.body.appendChild(hint);

        this.scheduleA11yUpdate();
    }

    scheduleA11yUpdate() {
        if (this.a11yTimer) return;
        this.a11yTimer = window.setTimeout(() => {
            this.a11yTimer = 0;
            this.refreshA11y();
        }, 100);
    }

    private refreshA11y() {
        if (!this.a11yEl || !this.renderer) return;
        const r = this.renderer;
        const lines: string[] = [];
        for (const row of r.scrollback) lines.push(rowToText(row));
        for (let i = 0; i < r.rows; i++) {
            const row = (r as unknown as { grid: Cell[][] }).grid[i];
            if (row) lines.push(rowToText(row));
        }
        while (lines.length > 0 && lines[lines.length - 1].trim() === '') lines.pop();
        this.a11yEl.textContent = lines.join('\n');
    }

    @bind
    private onBeforeUnload(e: BeforeUnloadEvent) {
        if (this.socket?.readyState === WebSocket.OPEN) {
            e.preventDefault();
            e.returnValue = '';
        }
    }

    private onResize() {
        if (!this.renderer || !this.parent) return;
        clearTimeout(this.resizeTimer);
        this.resizeTimer = window.setTimeout(() => {
            const sw = this.renderer!.scrollWrap;
            const { cols, rows } = this.renderer!.fit(
                sw.clientWidth || this.parent!.clientWidth,
                sw.clientHeight || this.parent!.clientHeight,
            );
            if (this.socket?.readyState === WebSocket.OPEN) {
                const msg = C2S.RESIZE_TERMINAL + JSON.stringify({ columns: cols, rows });
                this.socket.send(this.textEncoder.encode(msg));
            }
        }, 50);
    }

    private attachInput() {
        const canvas = this.renderer!.canvas;
        const scrollWrap = this.renderer!.scrollWrap;

        // TUI mouse cell coords (1-based, clamped to grid).
        const cellCoords = (e: MouseEvent) => {
            const r = canvas.getBoundingClientRect();
            const col = Math.min(this.renderer!.cols, Math.max(1, Math.floor((e.clientX - r.left) / this.renderer!.cellW) + 1));
            const row = Math.min(this.renderer!.rows, Math.max(1, Math.floor((e.clientY - r.top) / this.renderer!.cellH) + 1));
            return { col, row };
        };

        // Selection cell coords (0-based absLine, clamped to canvas).
        const selCoords = (e: MouseEvent) => {
            const r = canvas.getBoundingClientRect();
            const col = Math.max(0, Math.min(this.renderer!.cols - 1, Math.floor((e.clientX - r.left) / this.renderer!.cellW)));
            const viewRow = Math.max(0, Math.min(this.renderer!.rows - 1, Math.floor((e.clientY - r.top) / this.renderer!.cellH)));
            return { col, absLine: this.renderer!.viewFirstLine + viewRow };
        };

        const mods = (e: MouseEvent) => (e.shiftKey ? 4 : 0) | (e.altKey ? 8 : 0) | (e.ctrlKey ? 16 : 0);

        canvas.addEventListener('keydown', (e: KeyboardEvent) => {
            // Intercept Ctrl/Cmd+C as copy when a selection exists.
            if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'c' && !e.altKey && this.renderer!.sel) {
                e.preventDefault();
                navigator.clipboard.writeText(this.renderer!.buildSelectionText()).catch(() => {});
                return;
            }
            const s = encodeKey(e);
            if (s !== null) {
                e.preventDefault();
                this.sendInput(s);
            }
        });

        canvas.addEventListener('paste', (e: ClipboardEvent) => {
            const text = e.clipboardData?.getData('text');
            if (text) {
                e.preventDefault();
                this.sendInput(text);
            }
        });
        canvas.addEventListener('contextmenu', (e) => e.preventDefault());

        // Selection drag — on scrollWrap so it receives events whether the pointer
        // is over the canvas or the spacer padding above/below it.
        let selecting = false;
        scrollWrap.addEventListener('mousedown', (e: MouseEvent) => {
            if (e.button !== 0 || this.mouseMode !== 0) return;
            const { col, absLine } = selCoords(e);
            this.renderer!.sel = { aLine: absLine, aCol: col, fLine: absLine, fCol: col };
            selecting = true;
            e.preventDefault();
        });
        scrollWrap.addEventListener('mousemove', (e: MouseEvent) => {
            if (!selecting) return;
            const { col, absLine } = selCoords(e);
            this.renderer!.sel!.fLine = absLine;
            this.renderer!.sel!.fCol = col;
            this.renderer!.repaintViewport();
        });
        scrollWrap.addEventListener('mouseup', (e: MouseEvent) => {
            if (!selecting) return;
            selecting = false;
            const { col, absLine } = selCoords(e);
            const sel = this.renderer!.sel!;
            sel.fLine = absLine;
            sel.fCol = col;
            // Zero-width click: clear selection.
            if (sel.aLine === sel.fLine && sel.aCol === sel.fCol) {
                this.renderer!.sel = undefined;
            }
            this.renderer!.repaintViewport();
        });
        scrollWrap.addEventListener('mouseleave', () => { selecting = false; });

        // TUI mouse events — on canvas; only active when mouseMode !== 0.
        canvas.addEventListener('mousedown', (e: MouseEvent) => {
            canvas.focus();
            if (this.mouseMode === 0) return; // handled by scrollWrap above
            e.preventDefault();
            const btn = e.button === 0 ? 0 : e.button === 1 ? 1 : e.button === 2 ? 2 : 0;
            this.mouseDownBtn = btn;
            const { col, row } = cellCoords(e);
            this.sendInput(`\x1b[<${btn | mods(e)};${col};${row}M`);
        });
        canvas.addEventListener('mouseup', (e: MouseEvent) => {
            if (this.mouseMode === 0) return;
            e.preventDefault();
            const btn = this.mouseDownBtn >= 0 ? this.mouseDownBtn : (e.button === 0 ? 0 : e.button === 1 ? 1 : 2);
            this.mouseDownBtn = -1;
            const { col, row } = cellCoords(e);
            this.sendInput(`\x1b[<${btn | mods(e)};${col};${row}m`);
        });
        canvas.addEventListener('mousemove', (e: MouseEvent) => {
            if (this.mouseMode < 2) return;
            if (this.mouseMode === 2 && this.mouseDownBtn < 0) return;
            const btn = (this.mouseDownBtn >= 0 ? this.mouseDownBtn : 3) + 32;
            const { col, row } = cellCoords(e);
            this.sendInput(`\x1b[<${btn | mods(e)};${col};${row}M`);
        });

        // Wheel: forward as mouse codes when TUI owns the mouse; swallow on altscreen
        // without mouse mode (Case B); otherwise let the browser scroll natively (Case C).
        canvas.addEventListener('wheel', (e: WheelEvent) => {
            if (this.mouseMode !== 0) {
                e.preventDefault();
                const dir = e.deltaY > 0 ? 65 : 64;
                const { col, row } = cellCoords(e);
                this.sendInput(`\x1b[<${dir | mods(e)};${col};${row}M`);
                return;
            }
            if (this.renderer!.isAltScreen) {
                e.preventDefault(); // TUI fullscreen, no mouse mode — swallow
                return;
            }
            // Normal scroll: no preventDefault; browser scrolls the wrap natively.
        }, { passive: false });

        canvas.focus();
    }

    private sendInput(s: string) {
        if (this.socket?.readyState !== WebSocket.OPEN) return;
        const bytes = this.textEncoder.encode(s);
        const payload = new Uint8Array(bytes.length + 1);
        payload[0] = C2S.INPUT.charCodeAt(0);
        payload.set(bytes, 1);
        this.socket.send(payload);
    }

    @bind
    public connect() {
        this.socket = new WebSocket(this.options.wsUrl, ['tty']);
        this.socket.binaryType = 'arraybuffer';
        this.audio = new Audio('Bell.mp3');
        this.socket.addEventListener('open', () => this.onOpen());
        this.socket.addEventListener('message', (e) => this.onMessage(e));
        this.socket.addEventListener('close', (e) => this.onClose(e));
        this.socket.addEventListener('error', () => { this.doReconnect = false; });
    }

    private onOpen() {
        console.log('[tabsh] websocket connection opened');
        const { cols, rows } = this.renderer!;
        const msg = JSON.stringify({
            AuthToken: this.token,
            columns: cols,
            rows,
            sessionId: this.sessionId,
            cellDiff: true,
        });
        this.socket?.send(this.textEncoder.encode(msg));
        this.opened = true;
        this.doReconnect = this.reconnect;
        this.renderer?.focus();
    }

    private onClose(e: CloseEvent) {
        console.log(`[tabsh] websocket closed code=${e.code}`);
        if (e.code !== 1000 && this.doReconnect) {
            setTimeout(() => this.refreshToken().then(() => this.connect()), 500);
        } else if (this.closeOnDisconnect) {
            window.close();
        }
    }

    private onMessage(e: MessageEvent) {
        const buf = e.data as ArrayBuffer;
        const cmd = String.fromCharCode(new Uint8Array(buf, 0, 1)[0]);
        const data = buf.slice(1);
        switch (cmd) {
            case S2C.CELL_DIFF:
                this.renderer?.applyFrame(data);
                this.scheduleA11yUpdate();
                break;
            case S2C.SB_PUSH:
                this.renderer?.applySbPush(data);
                this.scheduleA11yUpdate();
                break;
            case S2C.SET_WINDOW_TITLE: {
                const title = this.textDecoder.decode(data);
                if (!this.titleFixed) {
                    this.currentTitle = title;
                    document.title = title;
                }
                break;
            }
            case S2C.SET_PREFERENCES:
                try {
                    const prefs = JSON.parse(this.textDecoder.decode(data));
                    this.applyPrefs(prefs);
                } catch { /* ignore */ }
                break;
            case S2C.SET_REATTACHED:
                // server follows with MOUSE_MODE + ALT_SCREEN + full repaint frame
                break;
            case S2C.MOUSE_MODE:
                this.mouseMode = new Uint8Array(data)[0] ?? 0;
                console.log(`[tabsh] mouse mode = ${this.mouseMode}`);
                break;
            case S2C.ALT_SCREEN:
                this.renderer?.setAltScreen((new Uint8Array(data)[0] ?? 0) === 1);
                break;
            case S2C.SET_APP_COMMAND:
            case S2C.SET_APP_FAVICON:
                break;
            default:
                console.warn(`[tabsh] unknown command: ${cmd}`);
        }
    }

    private applyPrefs(prefs: ClientOptions) {
        if (prefs.titleFixed) {
            this.titleFixed = prefs.titleFixed;
            document.title = prefs.titleFixed;
        }
        if (prefs.disableLeaveAlert) {
            window.removeEventListener('beforeunload', this.onBeforeUnload);
        }
        if (prefs.closeOnDisconnect) {
            this.closeOnDisconnect = true;
            this.reconnect = false;
            this.doReconnect = false;
        }
    }

    @bind
    public Bell() {
        this.audio?.play().catch(() => { /* ignore */ });
    }
}
