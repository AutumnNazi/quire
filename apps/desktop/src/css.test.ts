import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { clipItem } from "./clip-item";
import type { ClipItemContext } from "./clip-item";
import type { ClipSummary } from "./types";

/** 搭一条最小可用的剪藏和一个不做事的上下文。
 *  `clipItem` 搬出来之后能真跑了,`css.test.ts` 里那几条结构断言
 *  也就跟着升级成"搭一个出来看看"——比在源码里数字符可靠得多 */
function clip(over: Partial<ClipSummary> = {}): ClipSummary {
  return {
    id: "x",
    filename: "a.md",
    title: "标题",
    url: "https://a.com/1",
    site: "a.com",
    excerpt: null,
    clippedAt: "2026-01-01T00:00:00+08:00",
    read: false,
    archived: false,
    starred: false,
    progress: 0,
    tags: [],
    note: "",
    ...over,
  } as ClipSummary;
}

const ctx: ClipItemContext = {
  isActive: () => false,
  isSelected: () => false,
  onOpen: () => {},
  onContextMenu: () => {},
  onToggleRead: () => {},
  onToggleStar: () => {},
};

/**
 * `hidden` 属性到底管不管用,取决于有没有一条比类选择器更狠的规则。
 *
 * `hidden` 靠的是 UA 样式表里的 `display: none`,**特异性极低**。任何一条
 * `.类名 { display: flex }` 都能把它盖掉——那条属性就成了摆设。
 *
 * 这个坑已经踩了三次:`.toast` 补过 `[hidden]`,`.batch-bar` 又补过,
 * `.tag-bar` 和 `.batch-tag-panel` 漏了。后两者的后果是左侧栏底部常驻
 * 一条空白条(有边框、有底色、就是没有内容)。所以改成一条全局规则,
 * 并且把这件事钉住:再来第五个元素,这里会红。
 */

const mainSrc = readFileSync(resolve(process.cwd(), "src/main.ts"), "utf8");
/** 列表那一条的构造搬去了 `clip-item.ts`。样式/结构的检查得跟着走——
 *  留在 main.ts 里找的话,它会绿着,而它守的那个元素早就搬空了 */
const clipItemSrc = readFileSync(resolve(process.cwd(), "src/clip-item.ts"), "utf8");
const css = readFileSync(resolve(process.cwd(), "src/style.css"), "utf8");

/** 模板里带 `hidden` 属性、并且某个类在 CSS 里设了 `display` 的元素 */
function elementsThatCanIgnoreHidden(): { tag: string; className: string }[] {
  const found: { tag: string; className: string }[] = [];
  for (const m of mainSrc.matchAll(/<(\w+)[^>]*\bclass="([^"]+)"[^>]*\bhidden\b[^>]*>/g)) {
    const [, tag, classAttr] = m;
    for (const className of classAttr.split(/\s+/)) {
      const rule = new RegExp(`^\\.${className}\\s*\\{([^}]*)\\}`, "m").exec(css);
      if (rule && /display\s*:/.test(rule[1])) {
        found.push({ tag, className });
      }
    }
  }
  return found;
}

/**
 * 有没有一条**独立**的 `[hidden]` 规则。
 *
 * 这里必须卡住"选择器以 `[hidden]` 开头"。写成 `\[hidden\][^{]*\{` 的话,
 * `.toast[hidden] { display: none }` 也会被算成全局兜底——那条恰恰是
 * **按元素补的补丁**,正是这个测试要取代的东西。第一版就是这么写的,
 * 结果测试假绿,坑还在原地。
 *
 * `!important` 不是保险起见,是**必需**:这条规则写在文件第 31 行,
 * 而 `.batch-tag-panel`(74 行)、`.tag-bar`(92 行)、`.batch-bar`(798 行)、
 * `.toast`(363 行)全在它后面。同特异性(都是单类/单属性)时后来者胜,
 * 少了 `!important`,这条规则当下就是死的
 */
const hiddenRule = /^[^\S\n]*\[hidden\][^{]*\{([^}]*)\}/m.exec(css);
const hasGlobalHiddenRule = hiddenRule !== null && /display\s*:\s*none/.test(hiddenRule[1]);
const isImportant = hiddenRule !== null && /!\s*important/.test(hiddenRule[1]);

/**
 * 哪些"会盖掉 hidden"的规则排在这条兜底**后面**。
 *
 * 同特异性时后来者胜,所以排在后面的才是真正的威胁。这里算出来而不是写死
 * 行号:行号一动就废,顺序关系不会。
 */
function rulesAfterHiddenThatSetDisplay(): string[] {
  const found: string[] = [];
  if (hiddenRule === null) return found;
  const after = css.slice(hiddenRule.index + hiddenRule[0].length);
  for (const m of after.matchAll(/^\s*\.([\w-]+)\s*\{([^}]*)\}/gm)) {
    if (/display\s*:/.test(m[2])) found.push(m[1]);
  }
  return found;
}

/** 有一条兜住全局的 `[hidden]`,或者逐个给这个类补了 `[hidden]` */
function isCovered(className: string): boolean {
  if (hasGlobalHiddenRule) return true;
  return new RegExp(`^\\.${className}\\[hidden\\]\\s*\\{[^}]*display\\s*:\\s*none`, "m").test(css);
}

describe("hidden 属性", () => {
  it("有一条全局的兜底规则", () => {
    // 没有全局规则的话,每加一个带 display 的元素就得记得补一次,
    // 漏一次就是一个常驻的空白条或者常驻的弹窗
    expect(hasGlobalHiddenRule).toBe(true);
  });

  it("凡是设了 display 的元素,hidden 都真的管用", () => {
    const broken = elementsThatCanIgnoreHidden().filter((e) => !isCovered(e.className));
    expect(broken).toEqual([]);
  });

  /// 这条是把整个测试的必要性说清楚:少了兜底,批量标签面板就会
  /// 常驻在左侧栏底部——一个空 div,带着上边框和底色,永远关不掉
  it("批量标签面板的 hidden 真的管用", () => {
    const panel = elementsThatCanIgnoreHidden().find((e) => e.className === "batch-tag-panel");
    expect(panel, "模板里应当有带 hidden 的 .batch-tag-panel").toBeDefined();
    expect(isCovered("batch-tag-panel")).toBe(true);
  });

  it("兜底规则带 !important", () => {
    // 少这一个 !important,兜底当场变死代码。实测:去掉它,空白条立刻回来
    expect(isImportant, "全局 [hidden] 规则必须带 !important,否则压不过后面的类规则").toBe(true);
  });

  /**
   * 把"!important 是必需的"从注释里的断言变成能跑的断言。
   *
   * 只要后面还有任何一个设 display 的类规则,同特异性就归它赢,
   * 兜底必须靠 !important 撑着。谁要是把这条规则挪到文件末尾、
   * 或者哪天真把 !important 删了,这里会红。
   */
  it("!important 眼下确实是必需的,不是保险起见", () => {
    const later = rulesAfterHiddenThatSetDisplay();
    expect(later.length, "如果后面再没有设 display 的类规则,!important 就只是保险,可以撤了").toBeGreaterThan(0);
    expect(later).toContain("batch-tag-panel");
  });

  /**
   * **用了的类名都得有样式;反过来,写了样式就得有人用。**
   *
   * 这轮为搜索容错新加了四个类(近似命中的标、那行说明、搜不到那一屏、
   * 历史词条)。少写一条样式的后果不是"不好看",是**那个元素在界面上
   * 是个没框的裸文本**——用户看到「近似」两个字孤零零挂在标题旁边,
   * 不知道那是什么意思。而"CSS 里写着一个类名、模板里却没人用"更糟:
   * 那是给下一个人留的假线索,他会以为这个样式已经生效了
   */
  it("搜索容错那几个类都真的有样式,而且都真的被用上", () => {
    // **两个文件都要扫。** 列表那一条的构造在 `clip-item.ts` 里,
    // 只扫 main.ts 的话 `.clip-fuzzy` 会被判成"没人用"——
    // 而它恰恰是这四个里最要紧的一个
    const used = mainSrc + clipItemSrc;
    for (const cls of ["clip-fuzzy", "fuzzy-notice", "empty-search-miss", "search-miss-chip"]) {
      // **要匹配到选择器本身,不能只是"文本里出现过"。** 用 `toContain` 的话,
      // 把规则改名成 `.clip-fuzzy-typo` 照样命中 —— 而那个类名在模板里
      // 一个都没用,界面上就是一片裸的。这是条假绿,已经踩过一次
      expect(
        css,
        `CSS 里没有 .${cls} 这条规则,那个元素在界面上是裸的`,
      ).toMatch(new RegExp(`\\.${cls}\\s*(,|\\{|$)`, "m"));
      expect(
        used,
        `.${cls} 这个样式没人用,它是给下一个人留的假线索`,
      ).toMatch(new RegExp(`class(Name)?\\s*=\\s*"[^"]*\\b${cls}\\b`));
    }
  });

  /** `.clip-fuzzy` 得是**塞进标题里**的,不是挂在卡片外层。
   *
   * 挂在卡片外层会另起一行,把标题整体往下顶——扫列表时每一条都多一行
   * 无来由的小字,几百条的库就没法快速扫了 */
  it("近似命中的标是塞进标题里的", () => {
    // **搭一个真的出来,而不是在源码里数字符的位置。** 数位置只证明
    // "append 那行写在后面",证明不了它真进去了;真跑一遍,标是不是
    // 标题的后代节点一目了然
    const item = clipItem(clip({ starred: false }), null, { fuzzy: true }, ctx);
    const tag = item.querySelector(".clip-fuzzy");
    expect(tag, "近似命中得挂一个标").not.toBeNull();
    expect(tag!.parentElement?.className, "标不是标题的孩子,那它会另起一行").toContain(
      "clip-title",
    );
    // 反过来:标只能有一个,而且它的爹得是标题——挂在整行上就是另一个样子
    expect(item.querySelectorAll(".clip-fuzzy").length).toBe(1);
    expect(tag!.parentElement, "标的爹不是标题").toBe(item.querySelector(".clip-title"));
  });

  /**
   * **「未读」按钮上那个数字必须走 `unreadCount()`,不能在 `main.ts` 里
   * 自己数一遍。**
   *
   * 两份判定迟早对不上:改了一处另一处没跟上,用户看到的「未读 8」点进去
   * 却是 11 篇。而这种错**任何单测都抓不到**——两边各自都对,
   * `list.test.ts` 里那条一致性断言验的也是同一个函数
   */
  it("未读数走的是共享判定,不是各数一遍", () => {
    const fn = mainSrc.slice(
      mainSrc.indexOf("function renderUnreadBadge"),
      mainSrc.indexOf("function renderUnreadBadge") + 1200,
    );
    expect(fn, "源码里找不到 renderUnreadBadge").not.toBe("");
    expect(fn, "未读数没有走共享判定 unreadCount()").toContain("unreadCount(clips)");
    expect(
      fn,
      "renderUnreadBadge 里自己数了一遍 —— 两份判定迟早对不上,而用户看到的数字和点进去的条数对不上",
    ).not.toMatch(/clips\.filter\(/);
  });
});

/**
 * 阅读进度条的位置。它靠 sticky 贴在正文顶上,靠 DOM 顺序决定初始位置:
 * 标签、批注编辑器在它之上,正文在它之下——一条线把"我写的"和"我读的"
 * 分开。曾经靠负 margin 把它硬顶进标题区,那条线就横在标题和按钮中间,
 * 看着像渲染坏了。
 */
describe("阅读进度条的位置", () => {
  it("进度条是详情栏左边缘的竖线,不进滚动内容", () => {
    const src = readFileSync(resolve(process.cwd(), "src/main.ts"), "utf8");
    // 进度条**不在**详情滚动内容里。sticky 那条路试过两版都翻车:负 margin
    // 把线顶进标题区,去掉负 margin 后滚动时又压着穿过来的正文。它必须是
    // 详情栏左边缘一条竖线(阅读进度往下长,方向和滚动同向;栏内边距
    // 40px,线宽 3px,零遮挡)
    expect(
      src.indexOf('id="read-progress"'),
      "进度条得在骨架模板里、详情滚动区之外",
    ).toBeGreaterThan(-1);
    expect(
      src.indexOf('id="read-progress"'),
      "骨架里进度条得在详情滚动区之前",
    ).toBeLessThan(src.indexOf('class="detail-pane"'));
    expect(
      src.indexOf("progress, body)"),
      "进度条不许再被塞进详情滚动内容里",
    ).toBe(-1);
    // 竖线的长度是 height,不是 width——那是顶部横条的写法
    expect(src, "竖线进度用 height 记进度").toContain("progressBarEl.style.height");
    expect(src, "不许残留横条时代的 width 写法").not.toContain("progressBarEl.style.width");

    const css = readFileSync(resolve(process.cwd(), "src/style.css"), "utf8");
    const block = css.slice(
      css.indexOf(".read-progress {"),
      css.indexOf("}", css.indexOf(".read-progress {")),
    );
    expect(
      block,
      "进度条不许 sticky——悬在滚动内容上就会压住穿过来的正文",
    ).not.toMatch(/sticky|z-index|margin:\s*-/);
  });
});

/**
 * 双栏高度链。详情栏外包了一层 .detail-side(钉进度竖线用的),grid 子项
 * 的默认最小高度是内容高度——外壳没有 min-height:0 的话正文把它撑高,
 * 整页出现滚动条,左侧列表跟着一起滚。这是包层那轮真踩过的坑。
 */
describe("双栏布局的高度链", () => {
  it("详情外壳必须 min-height:0,不许把整页撑出滚动条", () => {
    const css = readFileSync(resolve(process.cwd(), "src/style.css"), "utf8");
    const start = css.indexOf(".detail-side {");
    const block = css.slice(start, css.indexOf("}", start));
    expect(block, "详情外壳缺 min-height:0,整页会被正文撑出滚动条").toContain(
      "min-height: 0",
    );
  });
});

/**
 * 本地化图片的路径基准。正文里的 `assets/...` 相对路径是相对 **md 所在
 * 目录**(clips/)成立的——md 阅读器能显示而软件裂图,就是因为两边
 * 用的基准不一样。渲染时必须拼 `库根/clips`,拿库根拼永远 404。
 */
describe("本地化图片的路径基准", () => {
  it("渲染时以 clips 目录为基准,不是库根", () => {
    const src = readFileSync(resolve(process.cwd(), "src/main.ts"), "utf8");
    expect(src, "clipsBase 得真的拼上 /clips").toContain("`" + "${root}/clips" + "`");
    const calls = src.match(/resolveAssetImages\(body, [^,]+, convertFileSrc\)/g) ?? [];
    expect(
      calls.length,
      "两处渲染(详情+重抓全文)都得转本地图片地址",
    ).toBe(2);
    expect(
      src.includes("resolveAssetImages(body, vaultRoot,"),
      "不许拿库根当基准——那是 404 裂图的旧写法",
    ).toBe(false);
  });
});

/**
 * 正文插图排版:img 默认行内,宽图顶左不居中。块级 + auto margin 是
 * 插图的通用排法。这个类是对外承诺过的正文样式,别顺手拆。
 */
describe("正文插图排版", () => {
  it("插图块级居中,并且不许超出正文宽度", () => {
    const css = readFileSync(resolve(process.cwd(), "src/style.css"), "utf8");
    const start = css.indexOf(".prose img {");
    const block = css.slice(start, css.indexOf("}", start));
    expect(block, "插图得 display:block 才能被 auto margin 居中").toContain("display: block");
    expect(block, "居中靠 auto margin").toContain("margin: 14px auto");
    expect(block, "图片不许把 42rem 的正文撑破").toContain("max-width: 100%");
  });
});
