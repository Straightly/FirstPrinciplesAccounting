import { readFileSync, readdirSync } from "node:fs";

const bundleUrl = new URL("../dist/workflow/workflow-react.js", import.meta.url);
const bundleDirUrl = new URL("../dist/workflow/", import.meta.url);
const files = readdirSync(bundleDirUrl);
if (files.length !== 1 || files[0] !== "workflow-react.js") {
  throw new Error("Workflow React bundle must be a single self-contained file");
}

const source = readFileSync(bundleUrl, "utf8");
if (/\b(?:import|from)\s*["'](?:react|react-dom)(?:\/[^"']*)?["']/.test(source)) {
  throw new Error("Workflow React bundle still imports npm packages");
}

const { React, createRoot } = await import(bundleUrl.href);
if (typeof React.createElement !== "function" || typeof createRoot !== "function") {
  throw new Error("Workflow React bundle does not export React and createRoot");
}
