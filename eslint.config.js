import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist/**", "src-tauri/**"] },
  ...tseslint.configs.recommended,
);
