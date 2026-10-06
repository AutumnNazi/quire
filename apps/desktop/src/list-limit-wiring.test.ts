import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * 列表上限接线验得对不对。
 *
 * `list.test.ts` 测的是截断算术,这里测的是**接上去没有**——那段代码
 * 藏在 `main.ts` 里(一 import 就 boot),只能读源码验结构。
 *
 * 要挡的回归都很具体,而且**运行时一声不吭**:
 *
 * - 少了「还有 N 篇」,上限就成了一次静默的数据丢失:用户明明存了八百篇,
 *   列表只列到两百,他会以为剩下的没了
 * - 换视图忘了把额度拨回去,用户切过去第一眼看到的是一个已经翻了两屏
 *   的列表,而他根本没在这个视图里翻过
 * - 有一条渲染路径漏了截断(比如回收站、搜索结果),那条路就还是老样子
 */
describe("列表上限接上了没有", () => {
  const mainSrc = readFileSync(resolve(process.cwd(), "src/main.ts"), "utf8");
  /** 取一整个函数体(**按大括号配平,不是数固定字数**)。
   *
   *  **数长度会吃过界**,把下一个函数也框进来——而那正是这条测试
   *  最容易被蒙混过去的地方:隔壁 `buildSearchMiss` 里有个
   *  `.slice(0, 5)`(搜索历史取前五个),框进来就会报"写死了条数" */
  const fnBody = (anchor: string): string => {
    const at = mainSrc.indexOf(anchor);
    if (at < 0) return "";
    const open = mainSrc.indexOf("{", at);
    let depth = 0;
    for (let i = open; i < mainSrc.length; i += 1) {
      if (mainSrc[i] === "{") depth += 1;
      else if (mainSrc[i] === "}") {
        depth -= 1;
        if (depth === 0) return mainSrc.slice(at, i + 1);
      }
    }
    return mainSrc.slice(at);
  };
  const calls = (name: string): number => mainSrc.split(name).length - 1;

  it("四条渲染路径都过同一道截断", () => {
    // 主列表、每周、搜索命中、回收站。**漏一条那条就还是老样子**——
    // 而"库大了列表卡"这件事,用户遇到的往往是主列表,不是回收站,
    // 所以漏了回收站不会有人报,但漏了主列表会
    //
    // 三处调用 + `appendPaged<T>(` 那个泛型定义里没有裸的 `appendPaged(`
    expect(calls("appendPaged("), "三条路径各调一次").toBe(3);
    for (const [where, anchor] of [
      ["主列表", "appendPaged(shown, (clip) => clipItem("],
      ["搜索命中", "appendPaged(results, (hit) =>"],
      ["回收站", "appendPaged(trash, (item) => trashItem(item))"],
    ] as const) {
      expect(mainSrc, `${where}那处得走 appendPaged`).toContain(anchor);
    }
    // 每周是分组渲染,套不进 appendPaged(周头不是条目),
    // 但它得自己拿 listWindow 算一遍
    const week = fnBody("function renderWeekList()");
    expect(week, "每周得有截断").toContain("listWindow(ordered, listLimit)");
    expect(week, "每周也得报还有多少").toContain("moreRow(hidden)");
  });

  it("截断走的是同一个函数,不是各写一份 slice", () => {
    // 各写各的(`slice(0, 200)` 散在四处)迟早分叉:改一处忘一处,
    // 界面上两处表现不一致,而没人知道哪个才是对的。
    // **只在渲染那几个函数里查**,别处的 slice 有别的用途
    // (搜索历史取前 5 个、预览取前 120 字),不归这条管
    for (const fn of [
      "function appendPaged<T>",
      "function renderWeekList()",
      "function renderHitList(",
      "function renderTrashList()",
    ]) {
      const body = fnBody(fn);
      expect(body.length, `没找到 ${fn}`).toBeGreaterThan(0);
      expect(body, `${fn} 里没有截断`).toMatch(/listWindow\(|appendPaged\(/);
      expect(body, `${fn} 里写死了条数`).not.toMatch(/slice\(0,\s*\d/);
    }
  });

  it("压着东西的时候会说一句", () => {
    // **这一条是上限能不能立住的另一半。** 少了它,用户存了八百篇
    // 只看到两百,会以为剩下的丢了
    expect(mainSrc, "得有「还有 N 篇」那一条").toContain("moreRow(hidden)");
    const row = fnBody("function moreRow(");
    expect(row, "得报还有几篇").toContain('t("list.more"');
    expect(row, "得给一个能往下看的按钮").toContain('t("list.more.action"');
    // 按钮真能放行,不是个摆设
    expect(row, "按钮得把额度加上去").toContain("listLimit += LIST_PAGE");
    expect(row, "加完得重画").toContain("renderList()");
  });

  it("换视图的四个入口都把额度拨回去了", () => {
    // 换筛选、换标签(标签栏)、换搜索词、从统计面板点标签。
    // **少一处就是一个说不清的界面**
    expect(calls("resetListWindow()"), "四处调用 + 一处定义").toBe(5);
    for (const [where, anchor] of [
      ["换筛选", "async function setFilter("],
      // **标签芯片要认准那一处。** `chip.addEventListener("click"` 有四个
      // 同名兄弟(搜索历史、导入结果、标签改名…),认第一个找到的话,
      // 这条测试会对着另一个芯片的代码说"拨回去了"
      ["换标签", "tagFilter = on ? null : item.tag;"],
      ["换搜索", "async function runSearch("],
      ["统计面板点标签", "          tagFilter = tag;"],
    ] as const) {
      const at = mainSrc.indexOf(anchor);
      expect(at, `找得到 ${where} 那处`).toBeGreaterThan(-1);
      const body = mainSrc.slice(at, at + 600);
      expect(body, `${where}没把额度拨回去`).toContain("resetListWindow()");
    }
  });

  it("上限只在真的变了视图时才拨,不是每次都拨", () => {
    // 挂在 `renderList()` 里(每次重画都拨)的话,用户点「显示更多」之后
    // 随手标一篇已读,列表立刻缩回一屏——而他刚才明明点过两次
    const renderList = fnBody("function renderList(): void {");
    expect(renderList, "renderList 里不能拨额度").not.toContain("resetListWindow()");
  });

  it("截断和键盘、全选用的是同一份 DOM", () => {
    // `visibleFilenames` 读的就是画出来的那些。**两者必须同源**:
    // 哪天有人让 `visibleFilenames` 改读 `clips`,Ctrl+A 就会把
    // 看不见的几千篇一起圈进去,而用户按下删除时屏幕上只有两百条
    const visible = fnBody("function visibleFilenames(): string[] {");
    expect(visible, "可见项得读 DOM").toContain("listEl.querySelectorAll");
    expect(visible, "可见项不能改读 clips").not.toContain("clips.");
  });
});
