import js from "@eslint/js";
import tseslint from "typescript-eslint";
export default tseslint.config(js.configs.recommended, ...tseslint.configs.strict, {
  rules: { "no-console": "warn", "@typescript-eslint/no-explicit-any": "error" },
});
