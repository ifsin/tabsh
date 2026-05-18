import path from 'path';
import { fileURLToPath } from 'url';
import { merge } from 'webpack-merge';
import ESLintPlugin from 'eslint-webpack-plugin';
import CopyWebpackPlugin from 'copy-webpack-plugin';
import HtmlWebpackPlugin from 'html-webpack-plugin';
import MiniCssExtractPlugin from 'mini-css-extract-plugin';
import CssMinimizerPlugin from 'css-minimizer-webpack-plugin';
import TerserPlugin from 'terser-webpack-plugin';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const devMode = process.env.NODE_ENV !== 'production';

const baseConfig = {
    context: path.resolve(__dirname, 'src'),
    entry: {
        app: './index.tsx',
    },
    output: {
        path: path.resolve(__dirname, 'dist'),
        filename: devMode ? '[name].js' : '[name].[contenthash].js',
    },
    module: {
        rules: [
            {
                test: /\.tsx?$/,
                use: 'ts-loader',
                exclude: /node_modules/,
            },
            {
                test: /\.s?[ac]ss$/,
                use: [
                    devMode ? 'style-loader' : MiniCssExtractPlugin.loader,
                    'css-loader',
                    {
                        loader: 'sass-loader',
                        options: {
                            api: 'modern',
                        },
                    },
                ],
            },
        ],
    },
    resolve: {
        extensions: ['.tsx', '.ts', '.js'],
    },
    plugins: [
        new ESLintPlugin({
            context: path.resolve(__dirname, '.'),
            extensions: ['js', 'jsx', 'ts', 'tsx'],
        }),
        new CopyWebpackPlugin({
            patterns: [{ from: './favicon.png', to: '.' }],
        }),
        new MiniCssExtractPlugin({
            filename: devMode ? '[name].css' : '[name].[contenthash].css',
            chunkFilename: devMode ? '[id].css' : '[id].[contenthash].css',
        }),
        new HtmlWebpackPlugin({
            inject: false,
            minify: {
                removeComments: true,
                collapseWhitespace: true,
            },
            title: 'ttyd - Terminal',
            template: './template.html',
        }),
    ],
    performance: {
        hints: false,
    },
};

const devConfig = {
    mode: 'development',
    devServer: {
        static: path.join(__dirname, 'dist'),
        compress: true,
        port: 9000,
        client: {
            overlay: {
                errors: true,
                warnings: false,
            },
        },
        proxy: [
            {
                context: ['/token', '/ws'],
                target: 'http://localhost:7681',
                ws: true,
            },
        ],
        webSocketServer: {
            type: 'sockjs',
            options: {
                path: '/sockjs-node',
            },
        },
    },
    devtool: 'inline-source-map',
};

const prodConfig = {
    mode: 'production',
    optimization: {
        minimizer: [new TerserPlugin(), new CssMinimizerPlugin()],
    },
    devtool: 'source-map',
};

export default merge(baseConfig, devMode ? devConfig : prodConfig);
