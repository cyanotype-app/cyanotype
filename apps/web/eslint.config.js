import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist", "node_modules"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    languageOptions: { globals: globals.browser },
    plugins: { "react-hooks": reactHooks },
    // Fetch-on-mount effects (auth, first page) legitimately set state once the request settles.
    rules: { ...reactHooks.configs.recommended.rules, "react-hooks/set-state-in-effect": "off" },
  },
);
