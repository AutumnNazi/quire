import { defineConfig } from "vite";
import { fileURLToPath } from "node:url";

/**
 * 只构建 content.ts,输出单文件 IIFE。
 *
 * MV3 的内容脚本**不支持 ES module**,必须是一个自包含的普通脚本。
 * 用多入口 + 默认 es 格式的话,Defuddle 会被 Rollup 抽成共享 chunk,
 * 内容脚本直接加载不了。所以这里锁死单入口、单文件。
 * manifest.json 与 background.js 不需要打包,放在 public/ 原样拷贝。
 */
export default defineConfig({
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "chrome116",
    rollupOptions: {
      input: fileURLToPath(new URL("./src/content.ts", import.meta.url)),
      output: {
        format: "iife",
        entryFileNames: "content.js",
      },
    },
  },
});
