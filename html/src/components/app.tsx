import { h, Component } from 'preact';
import { Terminal } from './terminal';
import type { ClientOptions, FlowControl } from './terminal/canvas';

const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';

// Parse /:app_id/:path from URL
// e.g. /zsh/Users/foo/bar → appId="zsh", cwd="/Users/foo/bar"
const pathParts = window.location.pathname.split('/').filter(Boolean);
const appId = pathParts[0] ?? 'term';
const cwdPath = pathParts.length > 1 ? '/' + pathParts.slice(1).join('/') : undefined;

// Handle ?new: clear sessionStorage and remove param
const params = new URLSearchParams(window.location.search);
if (params.has('new')) {
    sessionStorage.clear();
    params.delete('new');
    const search = params.toString() ? '?' + params.toString() : '';
    history.replaceState(null, '', window.location.pathname + search);
}

// Read ?cmd= query param (renamed from ?app=)
const initialCmd = params.get('cmd') ?? undefined;

// WS URL always /ws
const wsUrl = [protocol, '//', window.location.host, '/ws'].join('');

// Read injected config
const cfg = (window as any).__TABSH_CONFIG__ ?? {};
const theme = cfg.theme ?? {};
const termOptions = {
    fontSize: theme.font_size ?? 13,
    fontFamily: theme.font_family ?? 'Consolas,Liberation Mono,Menlo,Courier,monospace',
    theme: {
        foreground: theme.foreground ?? '#DFDBDD',
        background: theme.background ?? '#201F26',
        cursor: theme.cursor ?? '#FF60FF',
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

function onFavicon(url: string) {
    if (url) {
        setFavicon(url);
    } else {
        setFavicon('favicon.png');
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
                onFavicon={onFavicon}
            />
        );
    }
}
