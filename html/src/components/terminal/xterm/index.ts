import { bind } from 'decko';
import type { IDisposable, ITerminalOptions } from '@xterm/xterm';
import { Terminal } from '@xterm/xterm';
import { CanvasAddon } from '@xterm/addon-canvas';
import { ClipboardAddon } from '@xterm/addon-clipboard';
import { WebglAddon } from '@xterm/addon-webgl';
import { FitAddon } from '@xterm/addon-fit';
import { WebLinksAddon } from '@xterm/addon-web-links';
import { ImageAddon } from '@xterm/addon-image';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import { OverlayAddon } from './addons/overlay';
import { ZmodemAddon } from './addons/zmodem';

import '@xterm/xterm/css/xterm.css';

interface TtydTerminal extends Terminal {
    fit(): void;
}

declare global {
    interface Window {
        term: TtydTerminal;
    }
}

enum Command {
    // server side
    OUTPUT = '0',
    SET_WINDOW_TITLE = '1',
    SET_PREFERENCES = '2',
    SET_APP_COMMAND = '3',
    SET_REATTACHED = '4',
    SET_APP_FAVICON = '5',

    // client side
    INPUT = '0',
    RESIZE_TERMINAL = '1',
    PAUSE = '2',
    RESUME = '3',
    QUIT = '4',
}
type Preferences = ITerminalOptions & ClientOptions;

export type RendererType = 'dom' | 'canvas' | 'webgl';

export interface ClientOptions {
    rendererType: RendererType;
    disableLeaveAlert: boolean;
    disableResizeOverlay: boolean;
    enableZmodem: boolean;
    enableTrzsz: boolean;
    enableSixel: boolean;
    titleFixed?: string;
    isWindows: boolean;
    homeDir?: string;
    trzszDragInitTimeout: number;
    unicodeVersion: string;
    closeOnDisconnect: boolean;
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
    termOptions: ITerminalOptions;
}

function toDisposable(f: () => void): IDisposable {
    return { dispose: f };
}

function addEventListener(target: EventTarget, type: string, listener: EventListener): IDisposable {
    target.addEventListener(type, listener);
    return toDisposable(() => target.removeEventListener(type, listener));
}

export class Xterm {
    private disposables: IDisposable[] = [];
    private textEncoder = new TextEncoder();
    private textDecoder = new TextDecoder();
    private written = 0;
    private pending = 0;

    private terminal: Terminal;
    private fitAddon = new FitAddon();
    private overlayAddon = new OverlayAddon();
    private clipboardAddon = new ClipboardAddon();
    private webLinksAddon = new WebLinksAddon();
    private webglAddon?: WebglAddon;
    private canvasAddon?: CanvasAddon;
    private zmodemAddon?: ZmodemAddon;

    private socket?: WebSocket;
    private token: string;
    private sessionId: string;
    private opened = false;
    private titleFixed?: string;
    private homeDir?: string;
    private currentCwd?: string;
    private currentTitle?: string;
    private currentApp?: string;
    private resizeOverlay = true;
    private reconnect = true;
    private doReconnect = true;
    private closeOnDisconnect = false;
    private reattaching = false;
    private audio: HTMLAudioElement;

    private writeFunc = (data: ArrayBuffer) => this.writeData(new Uint8Array(data));

    private static generateSessionId(): string {
        const key = 'ttyd-session-id';
        const stored = sessionStorage.getItem(key);
        if (stored) return stored;
        const id = crypto.randomUUID();
        sessionStorage.setItem(key, id);
        return id;
    }

    private updateUrlParam(key: string, value: string) {
        const url = new URL(window.location.href);
        if (value && value !== '') {
            url.searchParams.set(key, value);
        } else {
            url.searchParams.delete(key);
        }
        window.history.replaceState(null, '', url.toString());
        this.setMetaProperty('og:url', url.toString());
        if (key === 'app') {
            this.currentApp = value || undefined;
            this.refreshDescription();
        }
    }

    private updateCwdPath(cwd: string) {
        const base = window.location.pathname.replace(/\/dir\/.*/, '').replace(/\/$/, '');
        const url = new URL(window.location.href);
        url.pathname = `${base}/dir${cwd}`;
        window.history.replaceState(null, '', url.toString());
        this.setMetaProperty('og:url', url.toString());
        this.refreshDescription();
    }

    private formatCwdTitle(cwd: string): string {
        if (this.options.clientOptions.isWindows) {
            const segments = cwd.replace(/\\/g, '/').split('/').filter(Boolean);
            if (segments.length <= 1) return cwd;
            return `${segments[segments.length - 1]}  ${segments.slice(0, -1).join('\\')}`;
        }

        const withTilde = this.homeDir && cwd.startsWith(this.homeDir) ? '~' + cwd.slice(this.homeDir.length) : cwd;

        const segments = withTilde
            .replace(/\/$/, '')
            .split('/')
            .filter(s => s !== '');
        if (segments.length === 0) return withTilde || '/';
        if (segments.length === 1) return withTilde;

        const currentDir = segments[segments.length - 1];
        const parentPath = withTilde.startsWith('~')
            ? '~/' + segments.slice(1, -1).join('/')
            : '/' + segments.slice(0, -1).join('/');
        return `${currentDir}  ${parentPath}`;
    }

    private static parseCwdFromPath(): string | null {
        const match = window.location.pathname.match(/\/dir(\/.*)/);
        return match ? decodeURIComponent(match[1]) : null;
    }

    private static parseAppFromQuery(): string | null {
        return new URLSearchParams(window.location.search).get('app');
    }

    private refreshDescription() {
        const desc = this.buildDescription();
        this.setMetaName('description', desc);
        this.setMetaProperty('og:description', desc);
    }

    private setMetaName(name: string, value: string) {
        let el = document.querySelector(`meta[name="${name}"]`) as HTMLMetaElement;
        if (!el) {
            el = document.createElement('meta');
            el.name = name;
            document.head.appendChild(el);
        }
        el.content = value;
    }

    private setMetaProperty(property: string, value: string) {
        let el = document.querySelector(`meta[property="${property}"]`) as HTMLMetaElement;
        if (!el) {
            el = document.createElement('meta');
            el.setAttribute('property', property);
            document.head.appendChild(el);
        }
        el.content = value;
    }

    private buildDescription(): string {
        const parts: string[] = [];
        if (this.currentApp) parts.push(`app: ${this.currentApp}`);
        if (this.currentCwd) parts.push(`cwd: ${this.currentCwd}`);
        return parts.length > 0 ? parts.join(', ') : 'Terminal session';
    }

    constructor(
        private options: XtermOptions,
        private sendCb: () => void
    ) {
        this.sessionId = Xterm.generateSessionId();
    }

    dispose() {
        for (const d of this.disposables) {
            d.dispose();
        }
        this.disposables.length = 0;
    }

    @bind
    private register<T extends IDisposable>(d: T): T {
        this.disposables.push(d);
        return d;
    }

    @bind
    public sendFile(files: FileList) {
        this.zmodemAddon?.sendFile(files);
    }

    @bind
    public async refreshToken() {
        try {
            const resp = await fetch(this.options.tokenUrl);
            if (resp.ok) {
                const json = await resp.json();
                this.token = json.token;
            }
        } catch (e) {
            console.error(`[ttyd] fetch ${this.options.tokenUrl}: `, e);
        }
    }

    @bind
    private onWindowUnload(event: BeforeUnloadEvent) {
        event.preventDefault();
        if (this.socket?.readyState === WebSocket.OPEN) {
            const message = 'Close terminal? this will also terminate the command.';
            event.returnValue = message;
            return message;
        }
        return undefined;
    }

    @bind
    private onWindowUnloadConfirmed() {
        if (this.socket?.readyState === WebSocket.OPEN) {
            this.socket.send(this.textEncoder.encode(Command.QUIT));
        }
    }

    @bind
    public open(parent: HTMLElement) {
        this.terminal = new Terminal(this.options.termOptions);
        const { terminal, fitAddon, overlayAddon, clipboardAddon, webLinksAddon } = this;
        window.term = terminal as TtydTerminal;
        window.term.fit = () => {
            this.fitAddon.fit();
        };

        terminal.loadAddon(fitAddon);
        terminal.loadAddon(overlayAddon);
        terminal.loadAddon(clipboardAddon);
        terminal.loadAddon(webLinksAddon);

        terminal.open(parent);
        fitAddon.fit();

        const termBuf = document.createElement('pre');
        termBuf.id = 'ttyd-buffer';
        termBuf.setAttribute('aria-label', 'terminal output');
        termBuf.style.cssText = 'position:absolute;left:-9999px;top:-9999px;width:1px;height:1px;overflow:hidden';
        document.body.appendChild(termBuf);

        terminal.onRender(() => {
            const lines: string[] = [];
            for (let i = 0; i < terminal.rows; i++) {
                lines.push(terminal.buffer.active.getLine(i)?.translateToString(true) ?? '');
            }
            termBuf.textContent = lines.join('\n');
        });

        const apiHint = document.createElement('div');
        apiHint.id = 'ttyd-api';
        apiHint.style.cssText = 'position:absolute;left:-9999px;top:-9999px;width:1px;height:1px;overflow:hidden';
        apiHint.textContent =
            'Terminal content API:\n' +
            '- Current viewport: #ttyd-buffer (DOM element)\n' +
            '- GET /content?lines=N  (last N raw lines, default 100)\n' +
            '- GET /content?blocks=N (last N command+output pairs, default 10)';
        document.body.appendChild(apiHint);

        terminal.parser.registerOscHandler(7, data => {
            try {
                const cwd = new URL(data).pathname;
                this.currentCwd = cwd;
                this.updateCwdPath(cwd);
                if (!this.titleFixed) {
                    const cwdFormatted = this.formatCwdTitle(cwd);
                    document.title = this.currentTitle ? `${this.currentTitle} | ${cwdFormatted}` : cwdFormatted;
                    this.setMetaProperty('og:title', document.title);
                    this.refreshDescription();
                }
            } catch (_) {
                /* malformed OSC 7 */
            }
            return true;
        });

        terminal.attachCustomKeyEventHandler((ev: KeyboardEvent) => {
            if (ev.type !== 'keydown') return true;

            // Cmd+K / Ctrl+K → Clear terminal
            if (ev.key === 'k' && (ev.metaKey || ev.ctrlKey)) {
                ev.preventDefault();
                terminal.clear();
                return false;
            }

            // Shift+Enter → Line break (newline without executing)
            if (ev.key === 'Enter' && ev.shiftKey) {
                ev.preventDefault();
                terminal.input('\n');
                return false;
            }

            return true;
        });
    }

    @bind
    private initListeners() {
        const { terminal, fitAddon, overlayAddon, register, sendData, Bell } = this;
        register(
            terminal.onTitleChange(data => {
                if (!this.titleFixed) {
                    this.currentTitle = data;
                    const title =
                        data && data !== ''
                            ? this.currentCwd
                                ? `${data} | ${this.formatCwdTitle(this.currentCwd)}`
                                : data
                            : this.currentCwd
                            ? this.formatCwdTitle(this.currentCwd)
                            : '';
                    document.title = title;
                    this.setMetaProperty('og:title', title);
                    this.refreshDescription();
                }
            })
        );
        register(terminal.onData(data => sendData(data)));
        register(
            terminal.onBell(() => {
                Bell();
            })
        );
        register(terminal.onBinary(data => sendData(Uint8Array.from(data, v => v.charCodeAt(0)))));
        register(
            terminal.onResize(({ cols, rows }) => {
                const msg = JSON.stringify({ columns: cols, rows: rows });
                this.socket?.send(this.textEncoder.encode(Command.RESIZE_TERMINAL + msg));
                if (this.resizeOverlay) overlayAddon.showOverlay(`${cols}x${rows}`, 300);
            })
        );
        register(
            terminal.onSelectionChange(() => {
                if (this.terminal.getSelection() === '') return;
                try {
                    document.execCommand('copy');
                } catch (e) {
                    return;
                }
                this.overlayAddon?.showOverlay('\u2702', 200);
            })
        );
        register(addEventListener(window, 'resize', () => fitAddon.fit()));
        register(addEventListener(window, 'beforeunload', this.onWindowUnload));
        register(addEventListener(window, 'unload', this.onWindowUnloadConfirmed));
    }

    @bind
    public Bell() {
        this.audio.play();
    }

    @bind
    public writeData(data: string | Uint8Array) {
        const { terminal, textEncoder } = this;
        const { limit, highWater, lowWater } = this.options.flowControl;

        this.written += data.length;
        if (this.written > limit) {
            terminal.write(data, () => {
                this.pending = Math.max(this.pending - 1, 0);
                if (this.pending < lowWater) {
                    this.socket?.send(textEncoder.encode(Command.RESUME));
                }
            });
            this.pending++;
            this.written = 0;
            if (this.pending > highWater) {
                this.socket?.send(textEncoder.encode(Command.PAUSE));
            }
        } else {
            terminal.write(data);
        }
    }

    @bind
    public sendData(data: string | Uint8Array) {
        const { socket, textEncoder } = this;
        if (socket?.readyState !== WebSocket.OPEN) return;

        if (typeof data === 'string') {
            const payload = new Uint8Array(data.length * 3 + 1);
            payload[0] = Command.INPUT.charCodeAt(0);
            const stats = textEncoder.encodeInto(data, payload.subarray(1));
            socket.send(payload.subarray(0, (stats.written as number) + 1));
        } else {
            const payload = new Uint8Array(data.length + 1);
            payload[0] = Command.INPUT.charCodeAt(0);
            payload.set(data, 1);
            socket.send(payload);
        }
    }

    @bind
    public connect() {
        this.socket = new WebSocket(this.options.wsUrl, ['tty']);
        this.audio = new Audio('Bell.mp3');
        const { socket, register } = this;

        socket.binaryType = 'arraybuffer';
        register(addEventListener(socket, 'open', this.onSocketOpen));
        register(addEventListener(socket, 'message', this.onSocketData as EventListener));
        register(addEventListener(socket, 'close', this.onSocketClose as EventListener));
        register(addEventListener(socket, 'error', () => (this.doReconnect = false)));
    }

    @bind
    private onSocketOpen() {
        console.log('[ttyd] websocket connection opened');

        const { textEncoder, terminal, overlayAddon } = this;
        const msg = JSON.stringify({
            AuthToken: this.token,
            columns: terminal.cols,
            rows: terminal.rows,
            sessionId: this.sessionId,
            ...(Xterm.parseCwdFromPath() ? { cwd: Xterm.parseCwdFromPath() } : {}),
            ...(Xterm.parseAppFromQuery() ? { app: Xterm.parseAppFromQuery() } : {}),
        });
        this.socket?.send(textEncoder.encode(msg));

        if (this.opened) {
            this.reattaching = sessionStorage.getItem('ttyd-session-id') !== null;
            if (!this.reattaching) {
                terminal.reset();
            }
            terminal.options.disableStdin = false;
            overlayAddon.showOverlay('Reconnected', 300);
        } else {
            this.opened = true;
        }

        this.doReconnect = this.reconnect;
        this.initListeners();
        terminal.focus();
    }

    @bind
    private onSocketClose(event: CloseEvent) {
        console.log(`[ttyd] websocket connection closed with code: ${event.code}`);

        const { refreshToken, connect, doReconnect, overlayAddon } = this;
        overlayAddon.showOverlay('Connection Closed');
        this.dispose();

        // 1000: CLOSE_NORMAL
        if (event.code !== 1000 && doReconnect) {
            overlayAddon.showOverlay('Reconnecting...');
            refreshToken().then(connect);
        } else if (this.closeOnDisconnect) {
            window.close();
        } else {
            const { terminal } = this;
            const keyDispose = terminal.onKey(e => {
                const event = e.domEvent;
                if (event.key === 'Enter') {
                    keyDispose.dispose();
                    overlayAddon.showOverlay('Reconnecting...');
                    refreshToken().then(connect);
                }
            });
            overlayAddon.showOverlay('Press ⏎ to Reconnect');
        }
    }

    @bind
    private parseOptsFromUrlQuery(query: string): Preferences {
        const { terminal } = this;
        const { clientOptions } = this.options;
        const prefs = {} as Preferences;
        const queryObj = Array.from(new URLSearchParams(query) as unknown as Iterable<[string, string]>);

        for (const [k, queryVal] of queryObj) {
            let v = clientOptions[k];
            if (v === undefined) v = terminal.options[k];
            switch (typeof v) {
                case 'boolean':
                    prefs[k] = queryVal === 'true' || queryVal === '1';
                    break;
                case 'number':
                case 'bigint':
                    prefs[k] = Number.parseInt(queryVal, 10);
                    break;
                case 'string':
                    prefs[k] = queryVal;
                    break;
                case 'object':
                    prefs[k] = JSON.parse(queryVal);
                    break;
                default:
                    console.warn(`[ttyd] maybe unknown option: ${k}=${queryVal}, treating as string`);
                    prefs[k] = queryVal;
                    break;
            }
        }

        return prefs;
    }

    @bind
    private onSocketData(event: MessageEvent) {
        const { textDecoder } = this;
        const rawData = event.data as ArrayBuffer;
        const cmd = String.fromCharCode(new Uint8Array(rawData)[0]);
        const data = rawData.slice(1);

        switch (cmd) {
            case Command.OUTPUT:
                this.writeFunc(data);
                break;
            case Command.SET_APP_COMMAND:
                this.updateUrlParam('app', textDecoder.decode(data).trim());
                break;
            case Command.SET_APP_FAVICON: {
                const path = textDecoder.decode(data).trim();
                const favicon = document.querySelector("link[rel='icon']") as HTMLLinkElement;
                if (favicon) {
                    if (!favicon.dataset.default) favicon.dataset.default = favicon.href;
                    favicon.href = path || favicon.dataset.default;
                }
                break;
            }
            case Command.SET_REATTACHED:
                this.reattaching = false;
                this.terminal.refresh(0, this.terminal.rows - 1);
                break;
            case Command.SET_PREFERENCES:
                this.applyPreferences({
                    ...this.options.clientOptions,
                    ...JSON.parse(textDecoder.decode(data)),
                    ...this.parseOptsFromUrlQuery(window.location.search),
                } as Preferences);
                break;
            default:
                console.warn(`[ttyd] unknown command: ${cmd}`);
                break;
        }
    }

    @bind
    private applyPreferences(prefs: Preferences) {
        const { terminal, fitAddon, register } = this;
        if (prefs.enableZmodem || prefs.enableTrzsz) {
            this.zmodemAddon = new ZmodemAddon({
                zmodem: prefs.enableZmodem,
                trzsz: prefs.enableTrzsz,
                windows: prefs.isWindows,
                trzszDragInitTimeout: prefs.trzszDragInitTimeout,
                onSend: this.sendCb,
                sender: this.sendData,
                writer: this.writeData,
            });
            this.writeFunc = data => this.zmodemAddon?.consume(data);
            terminal.loadAddon(register(this.zmodemAddon));
        }

        for (const [key, value] of Object.entries(prefs)) {
            switch (key) {
                case 'rendererType':
                    this.setRendererType(value);
                    break;
                case 'disableLeaveAlert':
                    if (value) {
                        window.removeEventListener('beforeunload', this.onWindowUnload);
                        console.log('[ttyd] Leave site alert disabled');
                    }
                    break;
                case 'disableResizeOverlay':
                    if (value) {
                        console.log('[ttyd] Resize overlay disabled');
                        this.resizeOverlay = false;
                    }
                    break;
                case 'disableReconnect':
                    if (value) {
                        console.log('[ttyd] Reconnect disabled');
                        this.reconnect = false;
                        this.doReconnect = false;
                    }
                    break;
                case 'enableZmodem':
                    if (value) console.log('[ttyd] Zmodem enabled');
                    break;
                case 'enableTrzsz':
                    if (value) console.log('[ttyd] trzsz enabled');
                    break;
                case 'trzszDragInitTimeout':
                    if (value) console.log(`[ttyd] trzsz drag init timeout: ${value}`);
                    break;
                case 'enableSixel':
                    if (value) {
                        terminal.loadAddon(register(new ImageAddon()));
                        console.log('[ttyd] Sixel enabled');
                    }
                    break;
                case 'closeOnDisconnect':
                    if (value) {
                        console.log('[ttyd] close on disconnect enabled (Reconnect disabled)');
                        this.closeOnDisconnect = true;
                        this.reconnect = false;
                        this.doReconnect = false;
                    }
                    break;
                case 'titleFixed':
                    if (!value || value === '') return;
                    console.log(`[ttyd] setting fixed title: ${value}`);
                    this.titleFixed = value;
                    document.title = value;
                    this.setMetaProperty('og:title', value);
                    this.refreshDescription();
                    break;
                case 'homeDir':
                    if (value) this.homeDir = value;
                    break;
                case 'isWindows':
                    if (value) console.log('[ttyd] is windows');
                    break;
                case 'unicodeVersion':
                    switch (value) {
                        case 6:
                        case '6':
                            console.log('[ttyd] setting Unicode version: 6');
                            break;
                        case 11:
                        case '11':
                        default:
                            console.log('[ttyd] setting Unicode version: 11');
                            terminal.loadAddon(new Unicode11Addon());
                            terminal.unicode.activeVersion = '11';
                            break;
                    }
                    break;
                default:
                    console.log(`[ttyd] option: ${key}=${JSON.stringify(value)}`);
                    if (terminal.options[key] instanceof Object) {
                        terminal.options[key] = Object.assign({}, terminal.options[key], value);
                    } else {
                        terminal.options[key] = value;
                    }
                    if (key.indexOf('font') === 0) fitAddon.fit();
                    break;
            }
        }
    }

    @bind
    private setRendererType(value: RendererType) {
        const { terminal } = this;
        const disposeCanvasRenderer = () => {
            try {
                this.canvasAddon?.dispose();
            } catch {
                // ignore
            }
            this.canvasAddon = undefined;
        };
        const disposeWebglRenderer = () => {
            try {
                this.webglAddon?.dispose();
            } catch {
                // ignore
            }
            this.webglAddon = undefined;
        };
        const enableCanvasRenderer = () => {
            if (this.canvasAddon) return;
            this.canvasAddon = new CanvasAddon();
            disposeWebglRenderer();
            try {
                this.terminal.loadAddon(this.canvasAddon);
                console.log('[ttyd] canvas renderer loaded');
            } catch (e) {
                console.log('[ttyd] canvas renderer could not be loaded, falling back to dom renderer', e);
                disposeCanvasRenderer();
            }
        };
        const enableWebglRenderer = () => {
            if (this.webglAddon) return;
            this.webglAddon = new WebglAddon();
            disposeCanvasRenderer();
            try {
                this.webglAddon.onContextLoss(() => {
                    this.webglAddon?.dispose();
                });
                terminal.loadAddon(this.webglAddon);
                console.log('[ttyd] WebGL renderer loaded');
            } catch (e) {
                console.log('[ttyd] WebGL renderer could not be loaded, falling back to canvas renderer', e);
                disposeWebglRenderer();
                enableCanvasRenderer();
            }
        };

        switch (value) {
            case 'canvas':
                enableCanvasRenderer();
                break;
            case 'webgl':
                enableWebglRenderer();
                break;
            case 'dom':
                disposeWebglRenderer();
                disposeCanvasRenderer();
                console.log('[ttyd] dom renderer loaded');
                break;
            default:
                break;
        }
    }
}
