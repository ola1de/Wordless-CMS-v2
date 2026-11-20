import { fileURLToPath } from "node:url";
import { dirname } from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const adminDir = dirname(fileURLToPath(import.meta.url));

/**
 * Wordless admin SPA build.
 *
 * Output goes to `admin/dist/admin/` (NOT `admin/dist/`) so that, with
 * wrangler `assets = { directory = "./admin/dist" }`, the SPA entry point
 * lands at `/admin/index.html` and does NOT shadow the public home page
 * at `/` (which would happen if dist/index.html existed).
 */
export default defineConfig({
  root: adminDir,
  plugins: [react()],
  base: "/admin/",
  build: {
    outDir: "dist/admin",
    emptyOutDir: true,
  },
});
