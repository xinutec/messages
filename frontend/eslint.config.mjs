// @ts-check
// ESLint flat config, type-aware: typescript-eslint recommendedTypeChecked and
// stylisticTypeChecked for bugs tsc misses (floating promises, unsafe `any`),
// plus the Angular rules (external templates and styles, template a11y).

import angular from "angular-eslint";
import tseslint from "typescript-eslint";

export default tseslint.config(
  // ts-rs output.
  { ignores: ["src/app/generated/**"] },
  {
    files: ["src/**/*.ts"],
    extends: [
      ...tseslint.configs.recommendedTypeChecked,
      ...tseslint.configs.stylisticTypeChecked,
      ...angular.configs.tsRecommended,
    ],
    languageOptions: {
      parserOptions: { projectService: true, tsconfigRootDir: import.meta.dirname },
    },
    processor: angular.processInlineTemplates,
    rules: {
      "@angular-eslint/component-max-inline-declarations": ["error", { template: 0, styles: 0 }],
      // `x as Shape` is the one way to fool DL-ANGULAR-STRINGIFIED-OBJECT's
      // template typing; narrow at the boundary instead.
      "@typescript-eslint/no-unsafe-type-assertion": "error",
      "@typescript-eslint/no-empty-function": "off",
    },
  },
  {
    // A test double asserted into its interface is the point of a double.
    files: ["src/**/*.spec.ts"],
    rules: {
      "@typescript-eslint/no-unsafe-type-assertion": "off",
    },
  },
  {
    // The e2e tree, type-aware for no-floating-promises: an unawaited
    // `route.fulfill(...)` still mocks the request, so nothing else notices.
    files: ["e2e/**/*.ts", "playwright.config.ts"],
    extends: [...tseslint.configs.recommendedTypeChecked, ...tseslint.configs.stylisticTypeChecked],
    languageOptions: {
      parserOptions: { projectService: true, tsconfigRootDir: import.meta.dirname },
    },
    rules: {
      "@typescript-eslint/no-unsafe-type-assertion": "error",
    },
  },
  {
    files: ["src/**/*.html"],
    extends: [...angular.configs.templateRecommended, ...angular.configs.templateAccessibility],
  },
);
