import { defineConfig } from '@hey-api/openapi-ts';

export default defineConfig({
  input: 'http://localhost:5173/openapi.json',
  logs: './build',
  output: {
    path: 'src/lib/client',
    postProcess: ['oxfmt']
  },
  plugins: [
    {
      enums: true,
      name: '@hey-api/typescript'
    },
    {
      name: '@hey-api/sdk',
      // Parse ISO date strings into `Date`s, matching the generated types.
      transformer: true
    },
    {
      bigInt: false,
      name: '@hey-api/transformers'
    },
    {
      baseUrl: '',
      name: '@hey-api/client-fetch',
      runtimeConfigPath: '$lib/backend/config'
    },
    {
      name: 'msw'
    }
  ]
});
