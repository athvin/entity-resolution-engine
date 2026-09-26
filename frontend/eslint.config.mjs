import { defineConfig, globalIgnores } from "eslint/config";
import nextPlugin from "@next/eslint-plugin-next";
import tseslint from "typescript-eslint";

export default defineConfig([
  globalIgnores([
    ".next/**",
    "node_modules/**",
    "coverage/**",
    "playwright-report/**",
    "test-results/**",
    "next-env.d.ts",
    "src/lib/api/schema.d.ts",
  ]),
  nextPlugin.configs["core-web-vitals"],
  ...tseslint.configs.strictTypeChecked.map((c) => ({
    ...c,
    files: ["**/*.ts", "**/*.tsx"],
  })),
  {
    files: ["**/*.ts", "**/*.tsx"],
    languageOptions: {
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    rules: {
      // The raw API field is semantically inverted for naive rendering (design doc §4.2);
      // state chips must derive from the resolve response `status` instead.
      "no-restricted-syntax": [
        "error",
        {
          selector: "Identifier[name='pending_until_next_reconcile']",
          message:
            "Do not read pending_until_next_reconcile — derive staged/applied/reconciled chips from the resolve response `status` (see docs/frontend-design.md §4.2).",
        },
      ],
      // Route handlers legitimately return `Response | NextResponse`.
      "@typescript-eslint/no-misused-promises": [
        "error",
        { checksVoidReturn: { attributes: false } },
      ],
    },
  },
  {
    files: ["**/*.mjs", "**/*.js"],
    extends: [tseslint.configs.disableTypeChecked],
  },
]);
