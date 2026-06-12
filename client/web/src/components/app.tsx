import { Component } from 'preact';
import { Terminal } from './terminal';
import type { ClientOptions, FlowControl } from './terminal/canvas';

const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';

const pathParts = window.location.pathname.split('/').filter(Boolean);
const appId = pathParts[0] ?? 'term';
const cwdPath = pathParts.length > 1 ? '/' + pathParts.slice(1).join('/') : undefined;

const params = new URLSearchParams(window.location.search);
if (params.has('new')) {
    sessionStorage.clear();
    params.delete('new');
    const search = params.toString() ? '?' + params.toString() : '';
    history.replaceState(null, '', window.location.pathname + search);
}

const initialCmd = params.get('cmd') ?? undefined;

const wsUrl = [protocol, '//', window.location.host, '/_ws'].join('');

const cfg = (window as any).__TABSH_CONFIG__ ?? {};
const bg = cfg.background ?? '#1E1E1E';
document.body.style.background = bg;
const termOptions = {
    fontSize: cfg.font_size ?? 13,
    fontFamily: cfg.font_family ?? 'Consolas,Liberation Mono,Menlo,Courier,monospace',
    lineHeight: cfg.line_height ?? 1.2,
    cursorStyle: (cfg.cursor_style ?? 'block') as 'block' | 'beam' | 'underline',
    cursorBlink: cfg.cursor_blink ?? false,
    theme: {
        foreground: cfg.foreground ?? '#D4D4D4',
        background: bg,
        cursor: cfg.cursor ?? '#AEAFAD',
    },
};

const clientOptions: ClientOptions = {
    disableLeaveAlert: false,
    disableResizeOverlay: false,
    closeOnDisconnect: false,
    isWindows: false,
    unicodeVersion: '11',
};

const flowControl: FlowControl = {
    limit: 100000,
    highWater: 10,
    lowWater: 4,
};

function setFavicon(url: string) {
    let el = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
    if (!el) {
        el = document.createElement('link');
        el.rel = 'icon';
        document.head.appendChild(el);
    }
    el.href = url;
}

function onCmd(cmd: string) {
    const p = new URLSearchParams(window.location.search);
    if (cmd) {
        p.set('cmd', cmd);
    } else {
        p.delete('cmd');
    }
    const search = p.toString() ? '?' + p.toString() : '';
    history.replaceState(null, '', window.location.pathname + search);
}

function onCwd(cwd: string) {
    const cleanCwd = cwd.startsWith('/') ? cwd.slice(1) : cwd;
    const search = window.location.search;
    history.replaceState(null, '', `/${appId}/${cleanCwd}${search}`);
}

function onFavicon(name: string) {
    if (!name) {
        setFavicon('/_fav/default.ico');
    } else if (/^(https?:|data:|\/)/.test(name)) {
        setFavicon(name);
    } else {
        setFavicon('/_fav/' + encodeURIComponent(name));
    }
}

export class App extends Component {
    render() {
        return (
            <Terminal
                id="terminal-container"
                wsUrl={wsUrl}
                clientOptions={clientOptions}
                termOptions={termOptions}
                flowControl={flowControl}
                cwd={cwdPath}
                appId={appId}
                cmd={initialCmd}
                onCmd={onCmd}
                onCwd={onCwd}
                onFavicon={onFavicon}
            />
        );
    }
}
