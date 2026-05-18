import { promises as fs } from 'fs';
import { dirname, resolve, join } from 'path';
import { fileURLToPath } from 'url';
import { gzipSync } from 'zlib';
import gulp from 'gulp';

const { task, series } = gulp;
const __dirname = dirname(fileURLToPath(import.meta.url));
const distDir = resolve(__dirname, 'dist');
const headerPath = resolve(__dirname, '../src/html.h');

const genHeader = (size, buf, len) => {
    let idx = 0;
    let data = 'unsigned char index_html[] = {\n  ';

    for (const value of buf) {
        idx++;

        const current = value < 0 ? value + 256 : value;

        data += '0x';
        data += (current >>> 4).toString(16);
        data += (current & 0xf).toString(16);

        if (idx === len) {
            data += '\n';
        } else {
            data += idx % 12 === 0 ? ',\n  ' : ', ';
        }
    }

    data += '};\n';
    data += `unsigned int index_html_len = ${len};\n`;
    data += `unsigned int index_html_size = ${size};\n`;
    return data;
};

const readDistFile = async filename => {
    return fs.readFile(join(distDir, filename));
};

task('clean', async () => {
    await fs.rm(distDir, { recursive: true, force: true });
});

task('inline', async () => {
    let html = await fs.readFile(join(distDir, 'index.html'), 'utf8');

    html = await replaceAsync(
        html,
        /<link inline rel="icon" type="([^"]+)" href="([^"]+)">/g,
        async (_match, type, href) => {
            const data = await readDistFile(href);
            return `<link rel="icon" type="${type}" href="data:${type};base64,${data.toString('base64')}">`;
        },
    );

    html = await replaceAsync(
        html,
        /<link inline rel="stylesheet" type="text\/css" href="([^"]+)">/g,
        async (_match, href) => {
            const css = await fs.readFile(join(distDir, href), 'utf8');
            return `<style type="text/css">${css}</style>`;
        },
    );

    html = await replaceAsync(
        html,
        /<script inline type="text\/javascript" src="([^"]+)"><\/script>/g,
        async (_match, src) => {
            const js = await fs.readFile(join(distDir, src), 'utf8');
            return `<script type="text/javascript">${js}</script>`;
        },
    );

    await fs.writeFile(join(distDir, 'inline.html'), html);
});

task(
    'default',
    series('inline', async () => {
        const html = await fs.readFile(join(distDir, 'inline.html'));
        const gzipped = gzipSync(html);
        await fs.rm(headerPath, { force: true });
        await fs.writeFile(headerPath, genHeader(html.length, gzipped, gzipped.length));
    }),
);

const replaceAsync = async (str, regex, asyncFn) => {
    const promises = [];

    str.replace(regex, (...args) => {
        promises.push(asyncFn(...args));
        return '';
    });

    const replacements = await Promise.all(promises);
    return str.replace(regex, () => replacements.shift());
};
