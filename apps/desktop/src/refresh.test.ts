import { describe, expect, it } from "vitest";
import { afterRefresh } from "./refresh";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

describe("刷新之后", () => {
  it("没在搜就重扫全库", () => {
    expect(afterRefresh("")).toBe("reload");
  });

  it("只有空格也当没在搜", () => {
    // 搜索框里手滑留了个空格,不该因此切到另一套行为
    expect(afterRefresh("   ")).toBe("reload");
  });

  it("正在搜就保持搜索状态", () => {
    // **这条是全部意义所在。** 用户输入 "rust",结果 8 条,随手按一下 F5——
    // 列表忽然变成全库几百条,而搜索框里还写着 "rust",
    // 他会以为搜索坏了,可能转头就去卸载软件
    expect(afterRefresh("rust")).toBe("same");
  });

  it("中文查询同样保持", () => {
    expect(afterRefresh("所有权")).toBe("same");
  });
});

/**
 * 上面那几条测的是判定,这里测的是**接上去没有**。
 *
 * `main.ts` 顶层就往 document 上挂东西,没法在单测里 import 进来,
 * 所以照 `css.test.ts` / `progress.test.ts` 的路子读源码验结构。
 * 真正要挡的回归是"有人嫌重扫慢,把 F5 或者聚焦重扫删了",
 * 而它删掉之后运行时完全正常,只是用户在编辑器里改完文件回来又看到旧的
 */
describe("刷新接对了没有", () => {
  const mainSrc = readFileSync(resolve(process.cwd(), "src/main.ts"), "utf8");

  it("工具栏上有刷新按钮", () => {
    expect(mainSrc, "模板里得有 #btn-refresh").toContain('id="btn-refresh"');
  });

  it("F5 能刷新", () => {
    expect(mainSrc, "F5 得接上").toMatch(/key\s*===\s*"F5"/);
  });

  it("窗口重新拿到焦点时会重扫", () => {
    // 用户在编辑器里改完文件切回来 —— 这是他最常遇到的情况。
    // 没有这条,他得记得自己按 F5,而他不会记得
    expect(mainSrc, "窗口聚焦时得重扫").toContain("window.addEventListener(\"focus\"");
  });

  it("刷新走的是重扫那条路,不是另写一套", () => {
    // 另写一套的话,过一阵两条路就会分叉(一个清了红条一个没清,
    // 一个刷了回收站一个没刷)。**只有一条路可走**
    expect(mainSrc).toContain("void refreshNow(");
  });

  it("F5 拦下了默认行为", () => {
    // **不拦的话 WebView 会整页重载** —— 搜索词、选中条目、正文滚动位置
    // 全没了。那不是刷新,那是重启,而用户按 F5 的动机恰恰是"轻量地看一眼"
    const f5 = mainSrc.slice(
      mainSrc.indexOf('e.key === "F5"'),
      mainSrc.indexOf('e.key === "F5"') + 200,
    );
    expect(f5, "F5 那一段里得有 preventDefault").toContain("e.preventDefault()");
  });

  it("刷新时不会把搜索结果冲掉", () => {
    // 判定对了但接线时无条件 `hits = null` 也一样坏:
    // 用户输入 "rust"、结果 8 条,按一下 F5 就变成几百条,
    // 而搜索框里还写着 "rust" —— 他会以为搜索坏了
    const body = mainSrc.slice(
      mainSrc.indexOf("async function refreshNow"),
      mainSrc.indexOf("async function refreshList"),
    );
    expect(body, "refreshNow 里不该无条件清掉 hits").toMatch(
      /if\s*\(afterRefresh\([^)]*\)\s*===\s*"reload"\)/,
    );
    expect(body).toContain("afterRefresh(searchEl.value)");
  });

  it("正在输入的时候不抢焦点", () => {
    // 用户在搜索框里打字,窗口焦点变化不该打断他
    expect(mainSrc).toContain("isTyping");
  });
});
