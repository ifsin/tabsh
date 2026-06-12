import { defineConfig, Plugin } from 'vite';
import preact from '@preact/preset-vite';
import { cpSync, copyFileSync, rmSync } from 'fs';
import { resolve, join } from 'path';
import { fileURLToPath } from 'url';

const __dirname = fileURLToPath(new URL('.', import.meta.url));

function embedPlugin(): Plugin {
    return {
        name: 'embed',
        apply: 'build',
        closeBundle() {
            const distDir = resolve(__dirname, 'dist');
            const embeddedDir = resolve(__dirname, '../../server/embedded');
            rmSync(embeddedDir, { recursive: true, force: true });
            cpSync(distDir, embeddedDir, { recursive: true });
            copyFileSync(resolve(__dirname, 'src/favicon.png'), join(embeddedDir, 'default.ico'));
        },
    };
}

export default defineConfig({
    plugins: [preact(), embedPlugin()],
    build: {
        outDir: 'dist',
        emptyOutDir: true,
        assetsDir: '',
        rollupOptions: {
            output: {
                assetFileNames: '[name][extname]',
                chunkFileNames: '[name].js',
                entryFileNames: '[name].js',
            },
        },
    },
    server: {
        port: 9000,
        proxy: {
            '/_ws': { target: 'ws://localhost:7681', ws: true },
            '/_fav': { target: 'http://localhost:7681' },
            '/token': { target: 'http://localhost:7681' },
        },
    },
});
