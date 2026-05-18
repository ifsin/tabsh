import { Component, h } from 'preact';
import { Xterm, XtermOptions } from './xterm';

interface Props extends XtermOptions {
    id: string;
}

export class Terminal extends Component<Props> {
    private container!: HTMLElement;
    private xterm: Xterm;

    constructor(props: Props) {
        super();
        this.xterm = new Xterm(props);
    }

    async componentDidMount() {
        await this.xterm.refreshToken();
        this.xterm.open(this.container);
        this.xterm.connect();
    }

    componentWillUnmount() {
        this.xterm.dispose();
    }

    render({ id }: Props) {
        return <div id={id} ref={c => { this.container = c as HTMLElement; }} />;
    }
}
