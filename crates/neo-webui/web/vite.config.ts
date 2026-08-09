import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Single-file bundle policy (handoff section 3.1): no source maps, no code
// splitting, no hashed names, no external URLs. The backend embeds exactly
// dist/index.html, dist/assets/neo-webui.js and dist/assets/neo-webui.css.
export default defineConfig({
  plugins: [react()],
  build: {
    sourcemap: false,
    cssCodeSplit: false,
    assetsInlineLimit: 0,
    rollupOptions: {
      output: {
        inlineDynamicImports: true,
        entryFileNames: "assets/neo-webui.js",
        chunkFileNames: "assets/neo-webui.js",
        assetFileNames: (assetInfo) => {
          if (assetInfo.name && assetInfo.name.endsWith(".css")) {
            return "assets/neo-webui.css";
          }
          return "assets/[name][extname]";
        },
      },
    },
  },
});
