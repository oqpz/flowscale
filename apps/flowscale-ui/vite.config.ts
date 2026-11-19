import { realpathSync } from "node:fs";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const rootDir = realpathSync(dirname(fileURLToPath(import.meta.url)));

export default defineConfig({
  root: rootDir,
  plugins: [react()],
  base: "./",
  build: {
    outDir: "dist",
    emptyOutDir: true
  }
});
