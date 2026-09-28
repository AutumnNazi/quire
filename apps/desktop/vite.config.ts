import { defineConfig } from "vite";

// Tauri 在 dev 模式下按硬编码端口拉这个 dev server,端口不能漂,
// 否则窗口会白屏报 ERR_CONNECTION_REFUSED。
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    // WebView2 运行时版本随 Windows 更新而变,保守目标避免语法降级问题
    target: "chrome110",
    sourcemap: false,
  },
});
