import { describe, expect, it } from "vitest";
import { clipItem } from "./clip-item";
import type { ClipItemContext } from "./clip-item";
import { LIST_PAGE, listWindow, selectClips } from "./list";
import type { ClipSummary } from "./types";

/**
 * 列表渲染的耗时实测。
 *
 * **这条测试是为了让列表上限那一项有据可依,不是凭感觉定的 200。**
 * 在此之前"一次画完会不会卡"从来没人量过,只能说"应该还行"——
 * 而"应该还行"在库涨到几千篇那天会被用户用卡顿打脸。
 *
 * 量的两段:
 *
 * 1. `selectClips` —— 纯数据,不碰 DOM
 * 2. `clipItem` 造节点 —— 这段是真正吃时间的地方,每条要建十来个元素
 *    加两三个监听
 *
 * **jsdom 不是浏览器,数字只能看量级和比例。** 绝对耗时跟 WebView2
 * 对不上(jsdom 建节点比真浏览器慢),但"200 条和 2000 条差多少倍"
 * 这件事是可移植的——而决定要不要设上限的正是这个比例。
 */

const ctx: ClipItemContext = {
  isActive: () => false,
  isSelected: () => false,
  onOpen: () => {},
  onContextMenu: () => {},
  onToggleRead: () => {},
  onToggleStar: () => {},
};

function makeClips(n: number): ClipSummary[] {
  const out: ClipSummary[] = [];
  for (let i = 0; i < n; i += 1) {
    // 时间往前铺,内容长短不一——全等长的数据量出来会偏乐观
    const day = String((i % 28) + 1).padStart(2, "0");
    const month = String((i % 12) + 1).padStart(2, "0");
    out.push({
      id: `id${i}`,
      filename: `2026-${month}-${day}-clip-${i}.md`,
      title: `第 ${i} 篇剪藏的标题,长短不一才像真的`,
      url: `https://example.com/post/${i}`,
      site: `example${i % 7}.com`,
      excerpt: i % 3 === 0 ? null : `这是第 ${i} 篇的摘要,用来看看这段文字有多长。`,
      clippedAt: `2026-${month}-${day}T0${i % 10}:30:00+08:00`,
      read: i % 4 === 0,
      archived: false,
      starred: i % 5 === 0,
      progress: i % 6 === 0 ? 0.42 : 0,
      tags: i % 3 === 0 ? ["待读", "长文"] : [],
      note: "",
    } as ClipSummary);
  }
  return out;
}

/** 造 `count` 条的 DOM 要多久(毫秒)。
 *
 *  **先热身,再量三轮取最小值。** 第一次跑永远最慢——JIT 还没编译,
 *  量出来的是"冷启动",不是"渲染成本"。第一版没热身,结果"库 200 篇"
 *  那一栏比"库 2000 篇"还慢 50%,看着像悖论,其实是先跑的那个在替
 *  后跑的付 JIT 的账。
 *
 *  取最小值而不是平均:异常值只会往上跑(GC、别的进程抢 CPU),
 *  最小值是最接近"这段代码本身要多久"的那个估计 */
function drawMs(clips: ClipSummary[], count = clips.length): number {
  const round = (): number => {
    const started = performance.now();
    const host = document.createElement("div");
    for (let i = 0; i < count; i += 1) {
      host.append(clipItem(clips[i], { text: "摘要", marks: [] }, {}, ctx));
    }
    const ms = performance.now() - started;
    // 别让这一批节点留在内存里影响下一轮
    host.remove();
    return ms;
  };
  round(); // 热身,不计
  return Math.min(round(), round(), round());
}

describe("列表渲染耗时", () => {
  const clips = makeClips(2000);

  it("筛一遍两千篇是纯数据,快得可以忽略", () => {
    // 这段是线性的 filter + sort。**它要是慢,列表上限也救不了**——
    // 那是另一个问题(排序算法),不是画布容量能盖住的
    selectClips(clips, "unread"); // 热身
    const started = performance.now();
    for (let i = 0; i < 20; i += 1) selectClips(clips, "unread");
    const ms = (performance.now() - started) / 20;
    console.log(`selectClips 两千篇(未读筛选+排序):${ms.toFixed(2)} ms/次`);
    expect(ms).toBeLessThan(50);
  });

  it("一屏 200 条的绘制耗时不随库的大小变化", () => {
    // **上限的意义就在这一条。** 库里 200 篇和 2000 篇,一屏该画多少
    // 还是多少——用户存得越多,单次渲染的开销不变
    const small = drawMs(makeClips(200));
    const big = drawMs(makeClips(2000), LIST_PAGE);
    console.log(`画 ${LIST_PAGE} 条(库 200 篇):${small.toFixed(1)} ms`);
    console.log(`画 ${LIST_PAGE} 条(库 2000 篇):${big.toFixed(1)} ms`);
    // 门限放得很宽,只为挡住数量级上的劣化。真跑起来是几十毫秒(jsdom)
    expect(small).toBeLessThan(3000);
    expect(big).toBeLessThan(3000);
  });

  it("不设上限的话,两千篇一次画完要慢一个数量级", () => {
    // **这条是上限存在的理由本身。** 不量它就只是"我觉得会卡";
    // 量出来才知道差多少——而差的那部分正是用户存到第三千篇那天
    // 每次点一下列表都要付一次的钱
    const capped = drawMs(clips, LIST_PAGE);
    const uncapped = drawMs(clips, clips.length);
    console.log(
      `${LIST_PAGE} 条:${capped.toFixed(1)} ms ／ 全部 ${clips.length} 条:${uncapped.toFixed(1)} ms`,
    );
    // 不写死倍数(那会随机器抖),只要求"画满确实更贵"——
    // 哪天有人把上限拆了,这个比例会先塌
    expect(uncapped).toBeGreaterThan(capped);
    // **超时放到 30 秒,不是因为这条慢,是因为它的工作量是量出来的那个数的四倍。**
    // `drawMs` 跑四轮(热身一轮 + 量三轮取最小值),两次调用合起来建约 8800 个
    // 节点;CI 上单轮就 1 秒,四轮直接撞上默认的 5 秒,被掐断的时候两个数
    // 其实都已经打印出来、断言也是成立的。余量不会掩盖劣化:断言是相对比较,
    // 真慢下来会先体现在打印的毫秒数上
  }, 30000);

  it("上限之外那部分确实没画,不是画了再藏", () => {
    // **这条比耗时数字重要。** 耗时随机器波动,而"DOM 里到底有几个节点"
    // 是硬的:`hidden` 那些必须连节点都没建。做成 `display:none` 的话
    // 省下的只是重排,建节点那份开销一分没少
    const host = document.createElement("div");
    // **走真的 `listWindow`,不另写一份截断。** 用副本测的话,
    // 哪天有人把真函数改成 `display:none` 方案,这条测试照样绿
    const { shown, hidden } = listWindow(clips, LIST_PAGE);
    for (const c of shown) host.append(clipItem(c, null, {}, ctx));
    expect(host.querySelectorAll(".clip")).toHaveLength(LIST_PAGE);
    expect(hidden).toBe(clips.length - LIST_PAGE);
    expect(clips.length).toBe(2000);
  });
});
