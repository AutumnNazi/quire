import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // Defuddle 靠可见性和尺寸判断正文,jsdom 没有排版引擎(尺寸全 0),
    // 所以这里只测不依赖布局的那部分:元数据挑选、HTML 拼装、实体转义。
    // 真实页面上的抽取效果仍要人工在浏览器里确认。
    environment: "jsdom",
    include: ["src/**/*.test.ts"],
  },
});
