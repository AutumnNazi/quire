import { describe, expect, it } from "vitest";
import { createProgressJudge, createProgressQueue } from "./progress";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

describe("阅读进度的提示", () => {
  it("存得上就一句话不说", () => {
    const j = createProgressJudge();
    expect(j.judge(true)).toBe("silent");
    expect(j.judge(true)).toBe("silent");
  });

  it("第一次存不上要提示", () => {
    // 这是全部意义所在:用户得知道自己没被记住
    expect(createProgressJudge().judge(false)).toBe("warn");
  });

  it("同一场故障只提示一次", () => {
    // 滚动停 1.5 秒就写一次,磁盘抖一下能连着失败十几次。
    // 每次都弹,那比不提示还糟——用户会开始无视所有提示
    const j = createProgressJudge();
    expect(j.judge(false)).toBe("warn");
    expect(j.judge(false)).toBe("silent");
    expect(j.judge(false)).toBe("silent");
  });

  it("好了要补一句,把这事结掉", () => {
    // 报完错一直不说"好了",用户会一直惦记着那个提示到底修没修好
    const j = createProgressJudge();
    j.judge(false);
    expect(j.judge(true)).toBe("recovered");
    expect(j.judge(true)).toBe("silent");
  });

  it("好了又坏了,重新提示一次", () => {
    // 中间好过,就不是同一场故障了。再不提示就等于一次漏报永久生效
    const j = createProgressJudge();
    j.judge(false);
    j.judge(true);
    expect(j.judge(false)).toBe("warn");
  });

  it("故障状态是记着的,不是靠上一条的结果推的", () => {
    const j = createProgressJudge();
    expect(j.warned).toBe(false);
    j.judge(false);
    expect(j.warned).toBe(true);
    j.judge(true);
    expect(j.warned).toBe(false);
  });
});

describe("待写的进度", () => {
  it("还没滚过就取,是空的", () => {
    // 打开一篇就切走,不该白写一次盘
    const q = createProgressQueue();
    expect(q.pending).toBe(false);
    expect(q.take()).toBeNull();
  });

  it("取了就没了,不能取两次", () => {
    // 取两次等于把同一篇的进度写两遍
    const q = createProgressQueue();
    q.put({ filename: "a.md", value: 0.4 });
    expect(q.take()).toEqual({ filename: "a.md", value: 0.4 });
    expect(q.take()).toBeNull();
  });

  it("连着滚只留最新那个位置", () => {
    // 用户要的是"我现在读到哪儿",不是滚动轨迹。留一串的话,
    // 写出去的就是几十毫秒前那个位置,倒退一点点
    const q = createProgressQueue();
    q.put({ filename: "a.md", value: 0.1 });
    q.put({ filename: "a.md", value: 0.5 });
    q.put({ filename: "a.md", value: 0.9 });
    expect(q.take()).toEqual({ filename: "a.md", value: 0.9 });
  });

  it("换篇之后待写的跟着换", () => {
    // 切文章时摘定时器,摘之前得先把上一篇那次交出去——
    // 丢了就是用户的位置停在原地,而进度条还显示着刚才那个位置
    const q = createProgressQueue();
    q.put({ filename: "a.md", value: 0.5 });
    expect(q.take()).toEqual({ filename: "a.md", value: 0.5 });
    q.put({ filename: "b.md", value: 0.1 });
    expect(q.take()).toEqual({ filename: "b.md", value: 0.1 });
  });
});

/**
 * 上面那 10 条测的是零件。这里测的是**接上去的那根线**。
 *
 * `main.ts` 顶层就往 document 上挂东西,没法在单测里 import 进来,
 * 所以照 `css.test.ts` 的路子直接读源码验结构。这不是耍花招——
 * 真正要挡的那个回归是"有人觉得多写一次盘浪费,把 flush 又删了",
 * 而它删掉之后运行时完全正常,只是用户的位置默默停在原地。
 */
describe("main.ts 接对了没有", () => {
  const mainSrc = readFileSync(resolve(process.cwd(), "src/main.ts"), "utf8");

  /** 抠出某个函数的函数体。 */
  const bodyOf = (name: string): string => {
    const start = mainSrc.indexOf(`function ${name}(`);
    expect(start, `main.ts 里应当有 ${name}`).toBeGreaterThan(-1);
    let depth = 0;
    let opened = false;
    for (let i = start; i < mainSrc.length; i++) {
      if (mainSrc[i] === "{") {
        depth++;
        opened = true;
      } else if (mainSrc[i] === "}") {
        depth--;
        if (opened && depth === 0) return mainSrc.slice(start, i + 1);
      }
    }
    throw new Error(`${name} 的函数体没闭合`);
  };

  it("摘进度监听之前先把那次写出去", () => {
    // 原来 detachProgress 只 clearTimeout:用户滚到一半点开下一篇,
    // 这一篇就永远停在原地,而进度条还显示着刚才那个位置
    expect(bodyOf("detachProgress")).toContain("flushPendingProgress()");
  });

  it("清定时器只在滚动和 flush 里,detach 自己不许清", () => {
    // 滚动那处是对的:每滚一下都要重启 1.5 秒的防抖。
    // detach 那处是错的:它一清,还挂在队列里的那次就没了。
    // 第一版这条写成了"全局只许出现一次",结果被 onScroll 那处合法清理打红——
    // 断言太宽等于没有断言
    const detach = bodyOf("detachProgress");
    expect(
      detach,
      "detachProgress 不该自己清定时器,该交给 flushPendingProgress",
    ).not.toContain("clearTimeout(progressTimer)");
    expect(bodyOf("flushPendingProgress"), "flush 自己得清一次定时器").toContain(
      "clearTimeout(progressTimer)",
    );
    expect(bodyOf("trackReadingProgress"), "滚动那处要留着,它负责重启防抖").toContain(
      "clearTimeout(progressTimer)",
    );
  });

  it("滚一下就进队列,定时器到点只负责取", () => {
    // 定时器回调里要是自己算 readingProgress,取到的是**那一刻**的位置,
    // 而 1.5 秒前那次滚动的位置就白丢了
    expect(bodyOf("trackReadingProgress")).toContain("progressQueue.put(");
    const timerCb = bodyOf("trackReadingProgress");
    expect(timerCb, "定时器到点应该调 flushPendingProgress").toContain("flushPendingProgress()");
  });

  it("存盘失败不再是空 catch,而是真的提示出去", () => {
    // 原来这里是 `catch {}` 配一句"不该打断",进度条显示着他读到哪儿,
    // 存不上他一点不知道。
    // **两条都要**:只查"不是空 catch"不够——把 catch 体换成 `void err;`
    // 照样是个空壳,而那一条只有 i18n 的"没有没人引用的键"能兜住
    const body = bodyOf("saveProgress");
    expect(body, "catch 体是空的,失败被整个吞了").not.toMatch(/catch\s*\{\s*\}/);
    expect(body, "存盘失败没接到提示上").toContain("toast.progressFailed");
    expect(body, "存盘好了要补一句,把故障结掉").toContain("toast.progressRecovered");
  });
});
