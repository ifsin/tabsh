import gts from 'gts';

export default [
    {
        ignores: ['dist/'],
    },
    ...gts,
    {
        files: ['.prettierrc.js', 'eslint.config.js', 'gulpfile.js', 'webpack.config.js'],
        languageOptions: {
            sourceType: 'module',
        },
    },
    {
        files: ['**/*.ts', '**/*.tsx'],
        languageOptions: {
            parserOptions: {
                jsxPragma: 'h',
            },
        },
    },
    {
        files: ['gulpfile.js', 'webpack.config.js'],
        languageOptions: {
            globals: {
                process: false,
            },
        },
        rules: {
            'n/no-unpublished-import': 'off',
        },
    },
];
