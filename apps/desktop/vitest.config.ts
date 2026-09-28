import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // 抽取链路要 DOM:DOMParser、Defuddle、DOMPurify 全都跑在真实文档上。
    // jsdom 不是浏览器,能过不代表在 WebView2 里表现一致——所以真站点的
    // 效果仍要在应用里人工确认,这里只锁住"逻辑分支没走错"。
    environment: "jsdom",
    include: ["src/**/*.test.ts"],
  },
});
