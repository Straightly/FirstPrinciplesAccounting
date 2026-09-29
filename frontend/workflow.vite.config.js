import { defineConfig } from "vite";

export default defineConfig({
  define: {
    "process.env.NODE_ENV": JSON.stringify("production")
  },
  build: {
    outDir: "dist/workflow",
    emptyOutDir: true,
    minify: true,
    lib: {
      entry: "workflow-react-entry.js",
      formats: ["es"],
      fileName: () => "workflow-react.js"
    }
  }
});
