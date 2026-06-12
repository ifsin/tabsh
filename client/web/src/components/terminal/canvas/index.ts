import init, { TabshTerminal, init_terminal } from '../../../wasm/tabsh_wasm';

// Server → client frame prefixes
const enum S2C {
    PTY = 0x00,
    REATTACHED = 0x01,
    STATE = 0x02,
}

// Client → server frame prefixes
const enum C2S {
    INPUT = 0x00,
    RESIZE = 0x01,
    INIT = 0x02,
    QUIT = 0x03,
    CLEAR = 0x04,
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

export interface TTYOptions {
    wsUrl: string;
    flowControl: FlowControl;
    clientOptions: ClientOptions;
    termOptions: {
        fontSize?: number;
        fontFamily?: string;
        lineHeight?: number;
        cursorStyle?: 'block' | 'beam' | 'underline';
        cursorBlink?: boolean;
        theme?: { foreground?: string; background?: string; cursor?: string };
    };
    cwd?: string;
    appId?: string;
    cmd?: string;
    onCmd?: (cmd: string) => void;
    onCwd?: (cwd: string) => void;
    onFavicon?: (url: string) => void;
}

export class TTY {
    private textDecoder = new TextDecoder();
    private socket?: WebSocket;
    private sessionId: string;
    private terminal?: TabshTerminal;
    private canvas?: HTMLCanvasElement;
    private parent?: HTMLElement;
    private resizeObs?: ResizeObserver;
    private reconnect = true;
    private doReconnect = true;
    private closeOnDisconnect = false;
    private titleFixed?: string;
    private currentTitle?: string;
    private resizeTimer = 0;
    private currentCmd = '';
    private retryDelay = 200;
    private retryCount = 0;
    private overlayEl?: HTMLDivElement;
    private wasmReady = false;
    private cols = 80;
    private rows = 24;
    private fontSize = 14;
    private cellW = 8;
    private cellH = 16;
    private measureCtx?: CanvasRenderingContext2D;

    private lineHeight = 1.2;

    private scrollWrap?: HTMLDivElement;
    private scrollInner?: HTMLDivElement;
    private textLayer?: HTMLDivElement;
    private lineEls: HTMLDivElement[] = [];
    private stickToBottom = true;
    private buttonDown = -1;
    private lastCell = { col: -1, row: -1 };
    private scrollSyncTimer = 0;
    private syncScheduled = false;
    private blinkTimer = 0;
    private blinkVisible = true;

    private static readonly MODE_MOUSE_CLICK = 1 << 3;
    private static readonly MODE_MOUSE_MOTION = 1 << 6;
    private static readonly MODE_MOUSE_DRAG = 1 << 13;
    private static readonly MODE_MOUSE_ANY = TTY.MODE_MOUSE_CLICK | TTY.MODE_MOUSE_MOTION | TTY.MODE_MOUSE_DRAG;

    private static generateSessionId(): string {
        const key = 'tabsh-session-id';
        const stored = sessionStorage.getItem(key);
        if (stored) return stored;
        const id = crypto.randomUUID();
        sessionStorage.setItem(key, id);
        return id;
    }

    constructor(
        private options: TTYOptions,
        private sendCb?: () => void,
    ) {
        this.sessionId = TTY.generateSessionId();
        this.fontSize = options.termOptions.fontSize ?? this.fontSize;
        this.lineHeight = options.termOptions.lineHeight ?? this.lineHeight;
    }

    private cursorShapeBits(): number {
        const style = this.options.termOptions.cursorStyle ?? 'block';
        if (style === 'beam') return 1;
        if (style === 'underline') return 2;
        return 0;
    }

    dispose() {
        try {
            this.socket?.close();
        } catch {}
        this.resizeObs?.disconnect();
        if (this.blinkTimer) clearInterval(this.blinkTimer);
        window.removeEventListener('beforeunload', this.onBeforeUnload);
        window.removeEventListener('unload', this.onUnload);
    }

    public sendFile = (_files: FileList) => {};

    public open = async (parent: HTMLElement) => {
        this.parent = parent;
        const bg = this.options.termOptions.theme?.background ?? '#000000';
        parent.style.width = '100%';
        parent.style.height = '100vh';
        parent.style.background = bg;

        const wrap = document.createElement('div');
        wrap.className = 'scroll-wrap';
        wrap.tabIndex = 0;
        wrap.style.cssText = `outline:none;background:${bg}`;

        const inner = document.createElement('div');
        inner.className = 'scroll-inner';

        this.canvas = document.createElement('canvas');
        this.canvas.style.cssText = 'display:block;position:sticky;top:0;left:0;z-index:0;will-change:transform';

        const textLayer = document.createElement('div');
        textLayer.style.cssText = [
            'position:absolute',
            'top:0',
            'left:0',
            'right:0',
            'z-index:1',
            'white-space:pre',
            'color:transparent',
            'font-family:monospace',
            `font-size:${this.fontSize}px`,
            `line-height:${this.cellH}px`,
            `--cell-h:${this.cellH}px`,
        ].join(';');

        inner.appendChild(this.canvas);
        inner.appendChild(textLayer);
        wrap.appendChild(inner);
        parent.appendChild(wrap);

        this.scrollWrap = wrap;
        this.scrollInner = inner;
        this.textLayer = textLayer;

        this.fitCanvas();

        this.resizeObs = new ResizeObserver(() => this.onResize());
        this.resizeObs.observe(parent);
        window.addEventListener('resize', () => this.onResize());
        this.watchDpr();
        window.addEventListener('beforeunload', this.onBeforeUnload);
        window.addEventListener('unload', this.onUnload);

        await init();
        const dpr = window.devicePixelRatio || 1;
        const theme = this.options.termOptions.theme ?? {};
        this.terminal = await init_terminal(
            this.canvas,
            this.cols,
            this.rows,
            this.fontSize * dpr,
            this.cursorShapeBits(),
            theme.foreground ?? '#D4D4D4',
            theme.background ?? '#1E1E1E',
            theme.cursor ?? '#AEAFAD',
        );
        this.wasmReady = true;

        if (this.options.termOptions.cursorBlink) {
            this.startBlink();
        }

        this.attachInput();
        wrap.focus();
    };

    private measureAdvance(): number {
        if (!this.measureCtx) {
            this.measureCtx = document.createElement('canvas').getContext('2d') ?? undefined;
        }
        if (!this.measureCtx) return this.fontSize * 0.6;
        this.measureCtx.font = `${this.fontSize}px monospace`;
        return this.measureCtx.measureText('M'.repeat(100)).width / 100;
    }

    private fitCanvas() {
        if (!this.canvas || !this.parent || !this.scrollWrap) return;
        const dpr = window.devicePixelRatio || 1;
        const w = this.parent.clientWidth || window.innerWidth;
        const h = this.parent.clientHeight || window.innerHeight;

        const advance = this.measureAdvance();
        const cellWDev = Math.max(1, Math.round(advance * dpr));
        const cellHDev = Math.max(1, Math.round(this.fontSize * this.lineHeight * dpr));
        this.cellW = cellWDev / dpr;
        this.cellH = cellHDev / dpr;

        const cols = Math.max(1, Math.floor(w / this.cellW));
        const rows = Math.max(1, Math.floor(h / this.cellH));
        const bw = cols * cellWDev;
        const bh = rows * cellHDev;

        const sizeChanged = this.canvas.width !== bw || this.canvas.height !== bh;
        const gridChanged = cols !== this.cols || rows !== this.rows;
        if (!sizeChanged && !gridChanged) return;

        this.cols = cols;
        this.rows = rows;
        this.canvas.width = bw;
        this.canvas.height = bh;
        this.canvas.style.width = `${bw / dpr}px`;
        this.canvas.style.height = `${bh / dpr}px`;
        this.scrollWrap.style.height = `${rows * this.cellH}px`;
        this.applyTextMetrics(advance);

        if (this.terminal && this.wasmReady) {
            this.terminal.resize(this.cols, this.rows, bw, bh, this.fontSize * dpr);
            this.stickToBottom = true;
            this.syncScrollAndText();
        }
        if (gridChanged && this.socket?.readyState === WebSocket.OPEN) {
            this.socket.send(TabshTerminal.resize_message(this.cols, this.rows));
        }
    }

    private watchDpr() {
        const mq = matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
        mq.addEventListener(
            'change',
            () => {
                this.fitCanvas();
                this.watchDpr();
            },
            { once: true },
        );
    }

    private applyTextMetrics(advance: number) {
        if (!this.textLayer) return;
        this.textLayer.style.fontSize = `${this.fontSize}px`;
        this.textLayer.style.letterSpacing = `${this.cellW - advance}px`;
        this.textLayer.style.lineHeight = `${this.cellH}px`;
        this.textLayer.style.setProperty('--cell-h', `${this.cellH}px`);
    }

    private mouseReporting(): boolean {
        if (!this.terminal) return false;
        return (this.terminal.mode_bits() & TTY.MODE_MOUSE_ANY) !== 0;
    }

    private syncScrollAndText() {
        if (!this.terminal || !this.textLayer || !this.scrollInner || !this.scrollWrap) return;
        const total = this.terminal.total_lines();
        const docHeight = total * this.cellH;
        this.scrollInner.style.height = `${docHeight}px`;
        this.textLayer.style.height = `${docHeight}px`;

        while (this.lineEls.length < total) {
            const el = document.createElement('div');
            el.className = 'term-line';
            this.textLayer.appendChild(el);
            this.lineEls.push(el);
        }
        while (this.lineEls.length > total) {
            this.lineEls.pop()!.remove();
        }

        const from = Math.max(0, total - this.rows - 1);
        for (let i = from; i < total; i++) {
            this.lineEls[i].textContent = this.terminal.line_text(i);
        }

        if (this.stickToBottom) {
            this.scrollWrap.scrollTop = this.scrollWrap.scrollHeight;
        }
        this.applyScrollOffset();
    }

    private resyncAllText() {
        if (!this.terminal) return;
        const total = this.terminal.total_lines();
        for (let i = 0; i < total && i < this.lineEls.length; i++) {
            this.lineEls[i].textContent = this.terminal.line_text(i);
        }
    }

    private applyScrollOffset() {
        if (!this.terminal || !this.scrollWrap || !this.canvas) return;
        const history = this.terminal.history_size();
        const offset = Math.max(0, Math.min(history, Math.round(history - this.scrollWrap.scrollTop / this.cellH)));
        this.terminal.set_display_offset(offset);
        const residual = this.scrollWrap.scrollTop - (history - offset) * this.cellH;
        this.canvas.style.transform = `translateY(${-residual}px)`;
        this.terminal.redraw();
    }

    private startBlink() {
        this.blinkTimer = window.setInterval(() => {
            if (!this.terminal || !this.wasmReady) return;
            if (!this.terminal.blinking_cursor()) {
                this.blinkVisible = true;
                return;
            }
            this.blinkVisible = !this.blinkVisible;
            this.terminal.redraw_ex(this.blinkVisible);
        }, 530);
    }

    private scheduleSync() {
        if (this.syncScheduled) return;
        this.syncScheduled = true;
        requestAnimationFrame(() => {
            this.syncScheduled = false;
            this.syncScrollAndText();
        });
    }

    private onScroll = () => {
        if (!this.scrollWrap) return;
        const atBottom =
            this.scrollWrap.scrollTop + this.scrollWrap.clientHeight >= this.scrollWrap.scrollHeight - this.cellH;
        this.stickToBottom = atBottom;
        this.applyScrollOffset();
        clearTimeout(this.scrollSyncTimer);
        this.scrollSyncTimer = window.setTimeout(() => this.resyncAllText(), 80);
    };

    private onResize() {
        if (!this.parent) return;
        clearTimeout(this.resizeTimer);
        this.resizeTimer = window.setTimeout(() => this.fitCanvas(), 50);
    }

    private getCell(e: MouseEvent): { col: number; row: number } {
        const rect = this.scrollWrap!.getBoundingClientRect();
        const col = Math.max(0, Math.min(this.cols - 1, Math.floor((e.clientX - rect.left) / this.cellW)));
        const row = Math.max(0, Math.min(this.rows - 1, Math.floor((e.clientY - rect.top) / this.cellH)));
        return { col, row };
    }

    private sendMouse(kind: number, button: number, e: MouseEvent) {
        if (!this.terminal || this.socket?.readyState !== WebSocket.OPEN) return;
        const { col, row } = this.getCell(e);
        const bytes = this.terminal.encode_mouse(kind, button, col, row, e.shiftKey, e.altKey, e.ctrlKey);
        if (bytes.length > 0) this.socket.send(bytes);
    }

    private attachInput() {
        const wrap = this.scrollWrap!;

        wrap.addEventListener('scroll', this.onScroll, { passive: true });

        wrap.addEventListener('mousedown', (e: MouseEvent) => {
            if (this.mouseReporting() && !e.shiftKey) {
                e.preventDefault();
                window.getSelection()?.removeAllRanges();
                this.buttonDown = e.button;
                this.sendMouse(0, e.button, e);
            }
            wrap.focus();
        });

        wrap.addEventListener('mousemove', (e: MouseEvent) => {
            if (!this.mouseReporting() || e.shiftKey) return;
            const bits = this.terminal!.mode_bits();
            const motion = (bits & TTY.MODE_MOUSE_MOTION) !== 0;
            const drag = (bits & TTY.MODE_MOUSE_DRAG) !== 0 && this.buttonDown >= 0;
            if (!motion && !drag) return;
            const { col, row } = this.getCell(e);
            if (col === this.lastCell.col && row === this.lastCell.row) return;
            this.lastCell = { col, row };
            this.sendMouse(2, this.buttonDown >= 0 ? this.buttonDown : 3, e);
        });

        wrap.addEventListener('mouseup', (e: MouseEvent) => {
            if (this.mouseReporting() && !e.shiftKey && this.buttonDown >= 0) {
                e.preventDefault();
                this.sendMouse(1, this.buttonDown, e);
            }
            this.buttonDown = -1;
        });

        wrap.addEventListener('contextmenu', (e: MouseEvent) => {
            if (this.mouseReporting() && !e.shiftKey) e.preventDefault();
        });

        wrap.addEventListener(
            'wheel',
            (e: WheelEvent) => {
                if (this.mouseReporting() && !e.shiftKey) {
                    e.preventDefault();
                    const kind = e.deltaY < 0 ? 3 : 4;
                    const notches = Math.max(1, Math.min(5, Math.round(Math.abs(e.deltaY) / this.cellH)));
                    for (let i = 0; i < notches; i++) this.sendMouse(kind, 0, e);
                }
            },
            { passive: false },
        );

        wrap.addEventListener('keydown', (e: KeyboardEvent) => {
            if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === 'r') {
                e.preventDefault();
                e.stopPropagation();
                if (this.socket?.readyState === WebSocket.OPEN) {
                    this.socket.send(new Uint8Array([C2S.QUIT]));
                }
                sessionStorage.removeItem('tabsh-session-id');
                this.doReconnect = false;
                window.location.reload();
                return;
            }
            if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k' && !e.altKey) {
                e.preventDefault();
                if (this.socket?.readyState === WebSocket.OPEN) {
                    this.socket.send(new Uint8Array([C2S.CLEAR]));
                }
                return;
            }
            if (!this.terminal || !this.wasmReady) return;
            const bytes = this.terminal.on_key(e.key, e.code, e.shiftKey, e.ctrlKey, e.altKey, e.metaKey);
            if (bytes.length > 0) {
                e.preventDefault();
                this.stickToBottom = true;
                if (this.socket?.readyState === WebSocket.OPEN) {
                    this.socket.send(bytes);
                }
            }
        });

        wrap.addEventListener('paste', (e: ClipboardEvent) => {
            const text = e.clipboardData?.getData('text');
            if (text && this.terminal && this.wasmReady) {
                e.preventDefault();
                const bracketed = `\x1b[200~${text}\x1b[201~`;
                const encoded = new TextEncoder().encode(bracketed);
                const frame = new Uint8Array(1 + encoded.length);
                frame[0] = C2S.INPUT;
                frame.set(encoded, 1);
                if (this.socket?.readyState === WebSocket.OPEN) {
                    this.socket.send(frame);
                }
            }
        });

        wrap.focus();
    }

    private showOverlay(msg: string) {
        if (!this.parent) return;
        if (!this.overlayEl) {
            const el = document.createElement('div');
            el.style.cssText = [
                'position:absolute',
                'inset:0',
                'z-index:100',
                'background:rgba(0,0,0,0.6)',
                'display:flex',
                'align-items:center',
                'justify-content:center',
                'color:#ccc',
                'font-family:monospace',
                'font-size:14px',
                'pointer-events:none',
            ].join(';');
            this.parent.style.position = 'relative';
            this.parent.appendChild(el);
            this.overlayEl = el;
        }
        this.overlayEl.textContent = msg;
        this.overlayEl.style.display = 'flex';
    }

    private hideOverlay() {
        if (this.overlayEl) this.overlayEl.style.display = 'none';
    }

    private showErrorScreen(msg: string) {
        this.hideOverlay();
        if (!this.parent) return;
        const el = document.createElement('div');
        el.style.cssText = [
            'position:absolute',
            'inset:0',
            'z-index:200',
            'background:#1e1e1e',
            'display:flex',
            'flex-direction:column',
            'align-items:center',
            'justify-content:center',
            'color:#f88',
            'font-family:monospace',
            'font-size:14px',
            'gap:16px',
        ].join(';');
        const text = document.createElement('div');
        text.textContent = msg;
        el.appendChild(text);
        this.parent.appendChild(el);
    }

    private showSessionEnded() {
        this.hideOverlay();
        if (!this.parent) return;
        const el = document.createElement('div');
        el.style.cssText = [
            'position:absolute',
            'inset:0',
            'z-index:200',
            'background:#1e1e1e',
            'display:flex',
            'flex-direction:column',
            'align-items:center',
            'justify-content:center',
            'color:#ccc',
            'font-family:monospace',
            'font-size:14px',
            'gap:16px',
        ].join(';');
        const text = document.createElement('div');
        text.textContent = 'Session ended';
        const btn = document.createElement('button');
        btn.textContent = 'Start new session';
        btn.style.cssText = [
            'padding:8px 16px',
            'background:#3a3a3a',
            'color:#ccc',
            'border:1px solid #666',
            'border-radius:4px',
            'font-family:monospace',
            'font-size:13px',
            'cursor:pointer',
        ].join(';');
        btn.addEventListener('click', () => window.location.reload());
        el.appendChild(text);
        el.appendChild(btn);
        this.parent.appendChild(el);
    }

    private onBeforeUnload = (e: BeforeUnloadEvent) => {
        if (this.currentCmd && this.socket?.readyState === WebSocket.OPEN) {
            e.preventDefault();
            (e as BeforeUnloadEvent & { returnValue: string }).returnValue = 'Are you sure?';
        }
    };

    private onUnload = () => {
        if (this.socket?.readyState === WebSocket.OPEN) {
            try {
                this.socket.send(new Uint8Array([C2S.QUIT]));
                this.socket.close(1000);
            } catch {}
        }
    };

    public connect = () => {
        this.socket = new WebSocket(this.options.wsUrl, ['tabsh']);
        this.socket.binaryType = 'arraybuffer';
        this.socket.addEventListener('open', () => this.onOpen());
        this.socket.addEventListener('message', (e) => this.onMessage(e));
        this.socket.addEventListener('close', (e) => this.onClose(e));
        this.socket.addEventListener('error', () => {
            this.doReconnect = false;
        });
    };

    private onOpen() {
        this.hideOverlay();
        this.retryDelay = 200;
        this.retryCount = 0;
        this.doReconnect = this.reconnect;

        const payload = new TextEncoder().encode(
            JSON.stringify({
                sessionId: this.sessionId,
                cols: this.cols,
                rows: this.rows,
                cwd: this.options.cwd ?? '',
                appId: this.options.appId ?? '',
                cmd: this.options.cmd ?? '',
            }),
        );
        const initMsg = new Uint8Array(1 + payload.length);
        initMsg[0] = C2S.INIT;
        initMsg.set(payload, 1);
        this.socket?.send(initMsg);
        this.canvas?.focus();
    }

    private onClose(e: CloseEvent) {
        if (e.code !== 1000 && this.doReconnect) {
            if (this.retryCount > 8) {
                this.showSessionEnded();
                return;
            }
            this.showOverlay('Reconnecting…');
            setTimeout(() => this.connect(), this.retryDelay);
            this.retryDelay = Math.min(this.retryDelay * 2, 5000);
            this.retryCount++;
        } else if (this.closeOnDisconnect) {
            window.close();
        }
    }

    private onMessage(e: MessageEvent) {
        const buf = e.data as ArrayBuffer;
        const view = new Uint8Array(buf);
        const cmd = view[0];
        const data = buf.slice(1);

        switch (cmd) {
            case S2C.PTY:
                if (this.terminal && this.wasmReady) {
                    this.terminal.on_pty_data(new Uint8Array(data));
                    this.handleWasmEvents();
                    this.scheduleSync();
                }
                break;

            case S2C.REATTACHED:
                break;

            case S2C.STATE: {
                try {
                    const s = JSON.parse(this.textDecoder.decode(data));
                    if ('error' in s) {
                        switch (s.error) {
                            case 'directory_not_found':
                                this.showErrorScreen(`Directory not found: ${s.path ?? ''}`);
                                this.doReconnect = false;
                                break;
                            case 'shell_not_found':
                                this.showErrorScreen(`Shell not available: ${s.shell ?? ''}`);
                                this.doReconnect = false;
                                break;
                            case 'session_already_attached':
                                sessionStorage.removeItem('tabsh-session-id');
                                this.sessionId = TTY.generateSessionId();
                                this.connect();
                                break;
                        }
                        break;
                    }
                    if ('title' in s) {
                        if (!this.titleFixed) {
                            this.currentTitle = s.title;
                            document.title = s.title;
                        }
                    }
                    if ('cmd' in s) {
                        this.currentCmd = s.cmd ?? '';
                        this.options.onCmd?.(s.cmd);
                    }
                    if ('cwd' in s) this.options.onCwd?.(s.cwd);
                    if ('favicon' in s) this.options.onFavicon?.(s.favicon);
                } catch {}
                break;
            }

            default:
                break;
        }
    }

    private handleWasmEvents() {
        if (!this.terminal) return;

        const title = this.terminal.pop_title();
        if (title !== undefined && !this.titleFixed) {
            this.currentTitle = title;
            document.title = title || document.title;
        }

        const cwd = this.terminal.pop_cwd();
        if (cwd !== undefined) {
            this.options.onCwd?.(cwd);
        }

        const cmd = this.terminal.pop_cmd();
        if (cmd !== undefined) {
            // Empty string means the shell returned to the prompt — clear the param.
            this.options.onCmd?.(cmd);
        }

        if (this.terminal.pop_bell()) {
            this.Bell();
        }
    }

    public Bell = () => {};
}
