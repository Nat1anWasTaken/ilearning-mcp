import { resolve } from "node:path"

import { backbone } from "@nat1an/mcp-backbone-dashboard/vite"
import { defineConfig } from "vite"

export default defineConfig({
  // `/files` serves course files to agents through short-lived links.
  plugins: [backbone({ proxy: ["/files"] })],
  resolve: {
    alias: { "@": resolve(import.meta.dirname, "./src") },
  },
})
