import { h, Component } from 'preact';
import { Terminal } from './terminal';
import type { ClientOptions, FlowControl } from './terminal/xterm';

const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
const path = window.location.pathname.replace(/\/dir\/.*/, '').replace(/[/]+$/, '');
const wsUrl = [protocol, '//', window.location.host, path, '/ws', window.location.search].join('');
const tokenUrl = [window.location.protocol, '//', window.location.host, path, '/token'].join('');

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

export class App extends Component {
    render() {
        return (
            <Terminal
                id="terminal-container"
                wsUrl={wsUrl}
                tokenUrl={tokenUrl}
                clientOptions={clientOptions}
                termOptions={termOptions}
                flowControl={flowControl}
            />
        );
    }
}
