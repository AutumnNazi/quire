import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * 正文外链接上了系统浏览器没有。
 *
 * 详情页就是 Quire 的整个界面。这里一条外链让 WebView 自己处理的话,
 * 默认行为是把窗口导航成那篇文章——用户眼里的「Quire 没了」。而这条
 * 回归极其安静:代码在、链接也在,只是点了之后整个界面被换掉。
 * 所以这里锁的是数据流:点 → 阻止默认导航 → 交给后端的 scheme 白名单。
 */
describe("正文外链接上了没有", () => {
  const mainSrc = readFileSync(resolve(process.cwd(), "src/main.ts"), "utf8");
  const apiSrc = readFileSync(resolve(process.cwd(), "src/api.ts"), "utf8");

  it("document 级有委托,而且调的是 openUrl", () => {
    // 正文每次重画都换一批节点,挨个绑监听既漏又忘解绑——委托只有一份
    expect(mainSrc, "得在 document 上做委托").toContain(
      'document.addEventListener("click"',
    );
    const at = mainSrc.indexOf('document.addEventListener("click"');
    const handler = mainSrc.slice(at, at + 900);
    expect(handler, "委托得接后端 openUrl").toContain("api.openUrl(href)");
  });

  it("只下放 http(s),页内的不拦", () => {
    // 锚点、相对路径是页面自己的东西,交给系统浏览器等于打开一个 404
    const at = mainSrc.indexOf('document.addEventListener("click"');
    const handler = mainSrc.slice(at, at + 900);
    expect(handler, "得有 scheme 判断").toMatch(/https\?:\\\/\\\//i);
    expect(handler, "得先阻止默认导航").toContain("e.preventDefault()");
    // preventDefault 必须在 openUrl 之前:顺序反了的话,导航已经发生,
    // 后端开不开浏览器都救不回来了
    expect(handler.indexOf("e.preventDefault()")).toBeLessThan(
      handler.indexOf("api.openUrl(href)"),
    );
  });

  it("后端会拒掉非网页地址,界面上说得出原因", () => {
    // 白名单在 Rust 侧(那儿才看得见 explorer 要执行什么),前端只管把
    // 失败说出来——报错键两边对得上才算接上
    const rust = readFileSync(
      resolve(process.cwd(), "src-tauri/src/lib.rs"),
      "utf8",
    );
    expect(rust, "得有 scheme 白名单").toContain('starts_with("https://")');
    expect(rust, "file:// 这类必须拒").toContain('"file:///C:/Windows/System32/cmd.exe"');
    expect(rust, "拒绝得有错误代号").toContain("openUrlRejected");
    expect(rust, "启动失败得有错误代号").toContain("openUrlFailed");
    const i18n = readFileSync(resolve(process.cwd(), "src/i18n.ts"), "utf8");
    expect(i18n, "代号得有文案").toContain('"error.openUrlRejected"');
    expect(i18n, "代号得有文案").toContain('"error.openUrlFailed"');
  });

  it("api.ts 里真的有那条 invoke", () => {
    expect(apiSrc).toContain('invoke<void>("open_url", { url })');
  });
});
