import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "jsdom",
    exclude: [
      "**/node_modules/**",
      "**/dist/**",
      "**/cypress/**",
      // Not `test?(s)/**`: tests/real/*.test.ts must run under vitest.
      "test?(s)/e2e/**",
      "**/*.e2e.{js,ts}",
      "**/tests/e2e/**",
      "**/e2e/**",
      "**/.{idea,git,cache,output,temp}/**",
      "**/{karma,rollup,webpack,vite,vitest,jest,ava,babel,nyc,cypress,tsup,build}.config.*",
    ],
    coverage: {
      provider: "v8",
      reporter: ["text", "json", "html"],
      reportsDirectory: "./coverage",
      exclude: [
        "coverage/**",
        "dist/**",
        "**/node_modules/**",
        "**/*.d.ts",
        "**/*.{test,spec}.{js,ts}",
        "**/__tests__/**",
        "tests/**",
        "src/main.ts",
        "src/vite-env.d.ts",
        "src/app.css",
        "stryker.conf.json",
        "reports/**",
      ],
      thresholds: {
        lines: 7,
        functions: 25,
        branches: 65,
        statements: 7,
      },
    },
  },
});
