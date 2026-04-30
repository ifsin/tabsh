import { h, Component } from 'preact';

import { Terminal } from './terminal';

import type { ITerminalOptions, ITheme } from '@xterm/xterm';
import type { ClientOptions, FlowControl } from './terminal/xterm';

const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
const path = window.location.pathname.replace(/\/dir\/.*/, '').replace(/[/]+$/, '');
const wsUrl = [protocol, '//', window.location.host, path, '/ws', window.location.search].join('');
const tokenUrl = [window.location.protocol, '//', window.location.host, path, '/token'].join('');
const clientOptions = {
    rendererType: 'webgl',
    disableLeaveAlert: false,
    disableResizeOverlay: false,
    enableZmodem: false,
    enableTrzsz: false,
    enableSixel: false,
    closeOnDisconnect: false,
    isWindows: false,
    unicodeVersion: '11',
} as ClientOptions;
const termOptions = {
    fontSize: 13,
    fontFamily: 'Consolas,Liberation Mono,Menlo,Courier,monospace',
    cursorBlink: true,
    cursorStyle: 'bar',
    drawBoldTextInBrightColors: false,
    theme: {
        foreground: '#DFDBDD',
        background: '#201F26',
        cursor: '#FF60FF',
        cursorAccent: '#201F26',
        selectionBackground: '#6B50FF',
        selectionForeground: '#F1EFEF',
        black: '#201F26',
        red: '#FF577D',
        green: '#12C78F',
        yellow: '#F5EF34',
        blue: '#00A4FF',
        magenta: '#FF60FF',
        cyan: '#0ADCD9',
        white: '#BFBCC8',
        brightBlack: '#4D4C57',
        brightRed: '#FF7F90',
        brightGreen: '#00FFB2',
        brightYellow: '#E8FE96',
        brightBlue: '#4FBEFE',
        brightMagenta: '#FF84FF',
        brightCyan: '#68FFD6',
        brightWhite: '#FFFAF1',
    } as ITheme,
    allowProposedApi: true,
} as ITerminalOptions;
const flowControl = {
    limit: 100000,
    highWater: 10,
    lowWater: 4,
} as FlowControl;

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
