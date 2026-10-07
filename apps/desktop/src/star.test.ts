import { describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { clipItem } from "./clip-item";
import type { ClipItemContext } from "./clip-item";
import type { ClipSummary } from "./types";

/**
 * 收藏那几处的**接线**。
 *
 * 这一组以前是读 `main.ts` 源码验结构(`clipItem` 埋在 `main.ts` 里,
 * 一 import 就 boot)。现在它搬进了 `clip-item.ts`,**能真跑一遍**——
 * 搭一条剪藏、模拟点那颗星,看行本身有没有被连带打开。
 * 源码正则只能证明"字符串出现了",跑一遍才能证明"点了真的不冒泡"。
 */

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
    starred: true,
    progress: 0,
    tags: [],
    note: "",
    ...over,
  } as ClipSummary;
}

function ctx(over: Partial<ClipItemContext> = {}): ClipItemContext {
  return {
    isActive: () => false,
    isSelected: () => false,
    onOpen: () => {},
    onContextMenu: () => {},
    onToggleRead: () => {},
    onToggleStar: () => {},
    ...over,
  };
}

describe("列表里的星", () => {
  it("只有收藏了的才画星", () => {
    // 全都画的话列表上就是一整列灰星星,没人知道哪些是开着的
    expect(clipItem(clip({ starred: true }), null, {}, ctx()).querySelector(".star-toggle"))
      .not.toBeNull();
    expect(clipItem(clip({ starred: false }), null, {}, ctx()).querySelector(".star-toggle"))
      .toBeNull();
  });

  it("星是个按钮,而且按下去是关掉它", () => {
    // 静态符号的话用户得先点开文章再翻开关,那叫让人多绕两步
    const el = clipItem(clip(), null, {}, ctx());
    const star = el.querySelector<HTMLButtonElement>(".star-toggle")!;
    expect(star.tagName).toBe("BUTTON");
    expect(star.getAttribute("aria-pressed")).toBe("true");
    expect(star.title).not.toBe("");
  });

  it("点星不会连带打开这一篇", () => {
    // **这条是真跑出来的,不是扫源码扫出来的。** 冒泡的话用户点一下
    // 取消收藏,结果那篇文章被打开了——他想取消收藏,却被甩进另一篇
    const onOpen = vi.fn();
    const onToggleStar = vi.fn();
    const el = clipItem(clip(), null, {}, ctx({ onOpen, onToggleStar }));
    el.querySelector<HTMLButtonElement>(".star-toggle")!.click();
    expect(onToggleStar).toHaveBeenCalledWith(
      expect.objectContaining({ filename: "a.md" }),
      false,
    );
    expect(onOpen, "星那一下不该冒泡到整行").not.toHaveBeenCalled();
  });

  it("点行的别处才打开这一篇", () => {
    // 上一条的"不冒泡"要能证伪:整行本身还开着才是对的
    const onOpen = vi.fn();
    const el = clipItem(clip(), null, {}, ctx({ onOpen }));
    el.querySelector<HTMLElement>(".clip-title")!.click();
    expect(onOpen).toHaveBeenCalled();
  });
});

describe("已读开关", () => {
  it("点已读不冒泡到整行", () => {
    // 同一个坑的另一处:不拦的话"标已读"会把那篇打开,
    // 而用户只是想把它从待读里划掉
    const onOpen = vi.fn();
    const onToggleRead = vi.fn();
    const el = clipItem(clip({ starred: false }), null, {}, ctx({ onOpen, onToggleRead }));
    el.querySelector<HTMLButtonElement>(".read-toggle")!.click();
    expect(onToggleRead).toHaveBeenCalledWith(expect.anything(), true);
    expect(onOpen).not.toHaveBeenCalled();
  });

  it("回收站那条路不给已读按钮", () => {
    // 给一堆删掉的剪藏挂个已读按钮,只会让人以为还能标
    const el = clipItem(clip(), null, { readToggle: false }, ctx());
    expect(el.querySelector(".read-toggle")).toBeNull();
  });
});

describe("收藏接的别处", () => {
  const mainSrc = readFileSync(resolve(process.cwd(), "src/main.ts"), "utf8");

  it("详情页也有开关", () => {
    expect(mainSrc).toContain("void toggleStar(clip, !clip.starred)");
  });

  it("批量条也能收藏", () => {
    // 多选的时候没法一篇篇点,那批量收藏就等于没有
    expect(mainSrc).toContain('id="batch-star"');
    expect(mainSrc).toContain("setClipFlagsBatch(names, read, archived, starred)");
  });

  it("收藏的四个值都真的往下传了", () => {
    // 只改签名不传值编译器不会报——收藏会静默失效
    const api = readFileSync(resolve(process.cwd(), "src/api.ts"), "utf8");
    expect(api).toContain("starred: starred ?? null,");
    expect(api.match(/starred: starred \?\? null,/g)?.length, "两个命令都得传").toBe(2);
  });
});