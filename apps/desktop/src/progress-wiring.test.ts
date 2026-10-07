import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * 进度条的语义接对了没有。
 *
 * 进度条显示的是**滚动位置**,不是"读完了没有"。这两个语义在不足一屏的
 * 文章上分道扬镳:完成度语义下短文打开就是 100%,而它没有可滚的空间,
 * 用户怎么拉线都不动——看起来就是进度条坏了。这里锁的是这次语义翻转
 * 之后的几条数据流,防的不是"算错",是有人把 0 改回 1、或者重算时顺手
 * 写了一次盘。
 */
describe("进度条是位置指示器,不是完成度", () => {
  const mainSrc = readFileSync(resolve(process.cwd(), "src/main.ts"), "utf8");

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

  it("不足一屏的文章线留在 0%,不假装读完", () => {
    // 完成度语义的 100% 在短文上就是"拉了没反应"的 bug:
    // 没有可滚空间,线怎么都不动,用户只会觉得软件坏了
    const fn = body("function readingProgress()");
    expect(fn.length, "没找到 readingProgress").toBeGreaterThan(0);
    expect(fn, "短文得归零").toMatch(/scrollable <= 8\) return 0/);
    expect(fn, "不许再返回 1 充当读完").not.toMatch(/scrollable <= 8\) return 1/);
  });

  it("布局长高只重画线,不写盘", () => {
    // 图片加载把正文撑高、窗口缩放,这些时刻可滚空间变了,线要跟上;
    // 但用户没有滚,**进度不该记一笔**——不然打开一篇文章什么都没干,
    // 硬盘上就多了一次写
    const fn = body("function syncProgressBarOnly()");
    expect(fn.length, "没找到 syncProgressBarOnly").toBeGreaterThan(0);
    expect(fn, "得重画线").toContain("progressBarEl.style.height");
    expect(fn, "不许碰进度队列").not.toContain("progressQueue.put");
    expect(fn, "不许调写盘").not.toContain("api.setClipProgress");
  });

  it("图片加载完会重算线,并校准还没被用户接管的位置恢复", () => {
    // 懒加载图片撑高正文是常态,不重算的话线停在加载前的旧值上,
    // 明明拉到了底,线还差一截;位置恢复同理——恢复用的是图片没加载
    // 时的 scrollHeight,布局一撑高位置就漂走(0.56 恢复到 75% 实测过)
    const at = mainSrc.indexOf('img.addEventListener("load"');
    expect(at, "图片得挂 load 重算").toBeGreaterThan(-1);
    // 传函数引用而不是箭头包一层:once 监听,引用就够了
    expect(mainSrc.slice(at, at + 120), "load 得调校准").toContain(
      "recalibrateRestore,",
    );
    const cal = body("function recalibrateRestore()");
    expect(cal, "校准只在用户没滚过时生效").toContain("pendingRestore != null");
    expect(cal, "校准走的是恢复函数").toContain("restoreSavedPosition()");
    expect(cal, "线也要跟着重画").toContain("syncProgressBarOnly()");
  });

  it("用户一滚,恢复就作废;恢复自己的滚动不算用户滚动", () => {
    // 用户动了滚动条,他的意图比记录大,校准必须停;但恢复本身也改
    // scrollTop 也触发 scroll——不垫标志,恢复第一步就会被自己冲掉
    const fn = body("function trackReadingProgress(filename: string, saved: number)");
    expect(fn, "恢复引发的滚动得被识别").toContain("restoringScroll");
    expect(fn, "识别后直接返回,不当用户滚动").toMatch(
      /if \(restoringScroll\) \{\s*restoringScroll = false;\s*return;/,
    );
    expect(fn, "用户滚动会作废待校准的恢复").toContain("pendingRestore = null;");
    // 摘监听时恢复也得作废,不然上一篇的恢复重放到这一篇上
    const detach = body("function detachProgress()");
    expect(detach, "摘监听得作废待校准").toContain("pendingRestore = null;");
  });

  it("写盘成功后,列表上的百分比要跟着动", () => {
    // **列表只在重画时读 `clips`,而滚动写盘不触发重画**——不同步的话
    // 百分比冻在扫描那一刻的值上:用户明明拉回了开头,列表还挂着 100%,
    // 怎么等都不动。这是"列表百分比不变化"的正主,不是竖线
    const save = body("async function saveProgress(filename: string, progress: number)");
    expect(save, "写盘成功得同步角标").toContain(
      "syncClipProgressBadge(filename, progress)",
    );
    const sync = body("function syncClipProgressBadge(filename: string, progress: number)");
    expect(sync, "内存里的 clips 得跟着改").toMatch(/clip\.progress\s*=\s*progress/);
    expect(sync, "列表上的角标得更新或移除").toContain(".clip-half");
    // 归零时角标得摘掉,和渲染条件(progress>0 才挂)对齐
    expect(sync, "归零得摘角标").toMatch(/else\s*\{\s*half\?\.remove\(\);?\s*\}/);
  });

  it("摘监听时线归零,回收站不继承上一篇的线", () => {
    // 回收站详情没有滚动监听,不归零的话它继承上一篇的线,
    // 拉到顶也不动——这是"拉到最开头还是 100%"的另一条真路径
    const fn = body("function detachProgress()");
    expect(fn, "摘监听得把线归零").toContain('progressBarEl.style.height = "0%"');
    // 归零在摘监听之后,renderDetail 路径紧接着 track 会重新赋值
    const removeAt = fn.indexOf('removeEventListener("scroll"');
    const resetAt = fn.indexOf('progressBarEl.style.height = "0%"');
    expect(resetAt, "归零得在摘监听之后").toBeGreaterThan(removeAt);
  });

  it("滚动的当下角标就跟上,不等写盘", () => {
    // 角标显示的是**阅读位置**,写盘另有 1.5 秒防抖。挂在写盘成功后面
    // 的话,连续滚动会把定时器一直往后推,直到切走才刷新——用户读着
    // 文章,列表上的百分比却停在打开时的值,像是没接上
    const fn = body("function trackReadingProgress(filename: string, saved: number)");
    const scrollAt = fn.indexOf("const onScroll = (): void => {");
    expect(scrollAt, "得有滚动监听").toBeGreaterThan(-1);
    const handler = fn.slice(scrollAt, fn.indexOf("};", fn.indexOf("flushPendingProgress()", scrollAt)));
    expect(handler, "角标值来自当前阅读位置").toMatch(
      /const value = readingProgress\(\)/,
    );
    expect(handler, "滚动时立即同步角标").toContain(
      "syncClipProgressBadge(filename, value)",
    );
    // 顺序:先同步显示,再进写盘队列——写盘可以晚,显示不能晚
    const badgeAt = handler.indexOf("syncClipProgressBadge");
    const queueAt = handler.indexOf("progressQueue.put");
    expect(badgeAt, "显示同步在写盘入队之前").toBeLessThan(queueAt);
  });

  it("同一个整数百分比不重复碰 DOM", () => {
    // 滚动事件一秒几十次,角标只在跨过整数百分点时才真的变——
    // 没有这条短路,2000 篇的库里每次滚动都全表 find + querySelector
    const sync = body("function syncClipProgressBadge(filename: string, progress: number)");
    const guard = sync.search(/Math\.round\(clip\.progress \* 100\) === shown/);
    const earlyReturn = sync.indexOf("return;", Math.max(0, guard));
    expect(guard, "得拿上一次显示的整数百分比比较").toBeGreaterThan(-1);
    expect(earlyReturn, "比较完得在同格里直接返回").toBeGreaterThan(guard);
  });
});
