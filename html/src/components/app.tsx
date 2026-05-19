import { h, Component } from 'preact';
import { Terminal } from './terminal';
import type { ClientOptions, FlowControl } from './terminal/canvas';

const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';

// Extract /dir/<cwd> from pathname, e.g. /dir/Users/foo/bar → cwd=/Users/foo/bar
const dirMatch = window.location.pathname.match(/\/dir(\/.*?)(?:\/ws)?$/);
const cwd = dirMatch ? dirMatch[1] : undefined;

// Strip /dir/... so WebSocket path is clean
const basePath = window.location.pathname.replace(/\/dir\/.*/, '').replace(/\/+$/, '');

// Read ?app=<cmd> query param
const params = new URLSearchParams(window.location.search);
const initialApp = params.get('app') ?? undefined;

// WS URL does not include ?app — server gets it from JSON_DATA handshake
const wsUrl = [protocol, '//', window.location.host, basePath, '/ws'].join('');

const clientOptions: ClientOptions = {
    disableLeaveAlert: false,
    disableResizeOverlay: false,
    closeOnDisconnect: false,
    isWindows: false,
    unicodeVersion: '11',
};

const termOptions = {
    fontSize: 13,
    fontFamily: 'Consolas,Liberation Mono,Menlo,Courier,monospace',
    theme: {
        foreground: '#DFDBDD',
        background: '#201F26',
        cursor: '#FF60FF',
    },
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

function onAppCommand(cmd: string) {
    const p = new URLSearchParams(window.location.search);
    if (cmd) {
        p.set('app', cmd);
    } else {
        p.delete('app');
    }
    const search = p.toString() ? '?' + p.toString() : '';
    history.replaceState(null, '', window.location.pathname + search);
}

function onAppFavicon(url: string) {
    if (url) {
        setFavicon(url);
    } else {
        // Restore default favicon when app exits
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
                cwd={cwd}
                app={initialApp}
                onAppCommand={onAppCommand}
                onAppFavicon={onAppFavicon}
            />
        );
    }
}
