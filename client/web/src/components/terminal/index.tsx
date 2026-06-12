import { Component, h } from 'preact';
import { TTY, TTYOptions } from './canvas';

interface Props extends TTYOptions {
    id: string;
}

export class Terminal extends Component<Props> {
    private container!: HTMLElement;
    private xterm: TTY;

    constructor(props: Props) {
        super();
        this.xterm = new TTY(props);
    }

    componentDidMount() {
        this.xterm.open(this.container);
        this.xterm.connect();
    }

    componentWillUnmount() {
        this.xterm.dispose();
    }

    render({ id }: Props) {
        return (
            <div
                id={id}
                ref={(c) => {
                    this.container = c as HTMLElement;
                }}
            />
        );
    }
}
