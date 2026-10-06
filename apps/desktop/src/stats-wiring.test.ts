import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * 「这库里有什么」接上了没有。
 *
 * `stats.test.ts` 测的是算术,这里测的是**面板和入口**。要挡的回归
 * 都很安静:数算对了、面板画出来了,只是没人能打开它,或者打开之后
 * 只有一堆数字、看完不知道该干什么——运行时不报任何错。
 */
describe("统计面板接上了没有", () => {
  const mainSrc = readFileSync(resolve(process.cwd(), "src/main.ts"), "utf8");

  /**
   * 取一整个函数体。
   *
   * **先按括号深度找到参数表结束,再找那个 `{`。** 直接找第一个 `{`
   * 会撞上 `{ name: string; count: number }` 这种类型注解,取出来的是
   * 签名不是函数体;而只找 `)` 也不够——参数里就有 `(name: string)`,
   * 从那儿往后找到的是 `=>` 里的那个 `{`。两种都试过,都错
   */
  const body = (anchor: string): string => {
    const at = mainSrc.indexOf(anchor);
    if (at < 0) return "";
    let depth = 0;
    let paramsClosed = -1;
    for (let i = at; i < mainSrc.length; i += 1) {
      if (mainSrc[i] === "(") depth += 1;
      else if (mainSrc[i] === ")") {
        depth -= 1;
        if (depth === 0) {
          paramsClosed = i;
          break;
        }
      }
    }
    if (paramsClosed < 0) return "";
    const open = mainSrc.indexOf("{", paramsClosed);
    depth = 0;
    for (let i = open; i < mainSrc.length; i += 1) {
      if (mainSrc[i] === "{") depth += 1;
      else if (mainSrc[i] === "}") {
        depth -= 1;
        if (depth === 0) return mainSrc.slice(at, i + 1);
      }
    }
    return mainSrc.slice(at);
  };

  it("工具栏上有入口", () => {
    // 没有按钮的话这个面板对用户来说就是不存在的东西
    expect(mainSrc, "工具栏上得有统计按钮").toContain('id="btn-stats"');
    expect(mainSrc, "按钮得接上打开").toMatch(
      /statsEl\.addEventListener\("click",[\s\S]{0,120}openStats\(\)/,
    );
  });

  it("Esc 关得掉", () => {
    // 一个关不掉的面板比没有面板更糟:它挡着正文,用户只能重启软件
    const at = mainSrc.indexOf('e.key === "Escape" && statsPanelEl');
    expect(at, "Esc 那条得认这个面板").toBeGreaterThan(-1);
    expect(mainSrc.slice(at, at + 160), "认得就得关").toContain("closeStats()");
  });

  it("数走的是那个纯函数,不是面板里现算一遍", () => {
    // 现算一遍就是第二份真相:加一个新指标时只改了一处,
    // 而两处不一致的时候用户信哪边都不对
    const fn = body("function renderStatsPanel(into: HTMLElement)");
    expect(fn.length, "没找到 renderStatsPanel").toBeGreaterThan(0);
    expect(fn, "得调 libraryStats").toContain("libraryStats(clips)");
    // 不许自己 filter 一遍——具体判据在 stats.ts 和 list.ts 里各只有一份
    expect(fn, "面板里不许自己数未读").not.toMatch(/filter\(.*\.read/);
    expect(fn, "面板里不许自己按月分组").not.toContain("groupByWeek");
  });

  it("算的是内存里那份,不是重扫磁盘", () => {
    // **这一页的意义就是随手看一眼。** 打开它要先等一次全库扫描的话,
    // 用户点之前得先想"值不值得等",那这一页就废了
    const fn = body("function renderStatsPanel(into: HTMLElement)");
    expect(fn, "不许在这里扫描磁盘").not.toMatch(/await\s+api\./);
  });

  it("标签那一栏点了能筛,不是纯展示", () => {
    // **只列数字不给动作的一页等于白做。** 用户看完"待读 37 篇"
    // 想知道是哪些,这一下点击就是唯一的出路
    const fn = body("function renderStatsPanel(into: HTMLElement)");
    expect(fn, "标签得能点").toContain("tagFilter = tag;");
    expect(fn, "点完得重画列表").toContain("renderList()");
    expect(fn, "点完得把面板收掉,不然它挡着刚筛出来的东西").toContain("closeStats()");
  });

  it("列不完的会说还有多少", () => {
    // 少一句「还有 N 个」,用户会以为他这辈子只在八个站点上存过东西
    const fn = body("function statsRank(");
    expect(fn, "排行得报还有多少没列").toContain("moreKey");
    expect(fn, "得真把那一句写出来").toContain('t(moreKey, { n: more })');
    const panel = body("function renderStatsPanel(into: HTMLElement)");
    expect(panel, "站点那栏得报剩下几个").toContain('"stats.moreSites"');
    expect(panel, "标签那栏得报剩下几个").toContain('"stats.moreTags"');
    expect(mainSrc, "有上限才谈得上剩下").toContain("STATS_RANK_LIMIT");
  });

  it("空库不显示一堆零", () => {
    // 四格全写 0 加一张空图,看着像坏了。**空库有它自己的说法**
    const fn = body("function renderStatsPanel(into: HTMLElement)");
    const emptyAt = fn.indexOf("stats.empty");
    const gridAt = fn.indexOf("stats-grid");
    expect(emptyAt, "得有专门给空库的那句话").toBeGreaterThan(-1);
    expect(gridAt, "四格在空库那句话后面").toBeGreaterThan(emptyAt);
  });

  it("零那一格不画柱子,不是画一条底线", () => {
    // **给它留底线的话,"这个月一篇没存"和"存了两篇"在图上长得一模一样**,
    // 而前者恰恰是这一页最该让人看见的事
    const fn = body("function renderStatsPanel(into: HTMLElement)");
    expect(fn, "零得单独判").toContain('m.count === 0 ? "0"');
  });
});
