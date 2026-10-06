import { describe, expect, it, vi } from "vitest";
import { undoTrashMany, undoableFiles } from "./undelete";
import type { BatchReport, ClipSummary } from "./types";

/** 造一篇剪藏摘要。批量撤销只要文件名对得上就够用了。 */
function clip(filename: string): ClipSummary {
  return {
    id: filename.replace(/\.md$/, ""),
    filename,
    title: `标题 ${filename}`,
    url: `https://example.com/${filename}`,
    site: "example.com",
    excerpt: null,
    clippedAt: "2026-09-28T14:30:00+08:00",
    read: false,
    archived: false,
    starred: false,
    progress: 0,
    tags: [],
    note: "",
  };
}

function report(succeeded: string[], failed: string[] = []): BatchReport {
  return {
    succeeded,
    failed: failed.map((filename) => ({
      filename,
      reason: { code: "vault.notFound", args: {} },
    })),
  };
}

describe("批量删除的撤销", () => {
  it("只把真删掉的那几篇算成可撤销", () => {
    // 失败的 2 篇压根没被删,塞进撤销名单的话,用户一点撤销,
    // 后端就得为它们各报一次"放不回来"——那是在骗他
    expect(undoableFiles(report(["a.md", "b.md", "c.md"], ["d.md"]))).toEqual([
      "a.md",
      "b.md",
      "c.md",
    ]);
  });

  it("一篇没删成就没有可撤销的", () => {
    expect(undoableFiles(report([], ["a.md", "b.md"]))).toEqual([]);
  });

  it("全删成了就全都可撤销", () => {
    expect(undoableFiles(report(["a.md", "b.md"]))).toEqual(["a.md", "b.md"]);
  });

  it("撤销拿回放回来的剪藏摘要", async () => {
    const restore = vi.fn(async (names: string[]) => ({
      report: report(names),
      clips: names.map(clip),
    }));

    const out = await undoTrashMany(["a.md", "b.md"], restore);

    expect(out.restored.map((c) => c.filename)).toEqual(["a.md", "b.md"]);
    expect(out.failed).toEqual([]);
    expect(restore).toHaveBeenCalledWith(["a.md", "b.md"]);
  });

  it("撤销有一篇放不回来时,失败要单独带出来", async () => {
    // 只弹一句"已撤销"的话,用户会以为那 2 篇也回来了,回头找不到
    const restore = vi.fn(async (names: string[]) => ({
      report: report([names[0]], [names[1], names[2]]),
      clips: [clip(names[0])],
    }));

    const out = await undoTrashMany(["a.md", "b.md", "c.md"], restore);

    expect(out.restored.map((c) => c.filename)).toEqual(["a.md"]);
    expect(out.failed.map((f) => f.filename)).toEqual(["b.md", "c.md"]);
    expect(out.failed[0].reason.code).toBe("vault.notFound");
  });

  it("撤销整批炸了要往上抛,由界面弹红条", async () => {
    // 网盘掉线那种整个请求失败,不能吞成"撤销了但什么也没变"
    const restore = vi.fn(async () => {
      throw { code: "vault.unavailable", args: {} };
    });

    await expect(undoTrashMany(["a.md"], restore)).rejects.toThrow();
  });

  it("没东西可撤销时压根不调后端", async () => {
    const restore = vi.fn();
    const out = await undoTrashMany([], restore);
    expect(restore).not.toHaveBeenCalled();
    expect(out.restored).toEqual([]);
    expect(out.failed).toEqual([]);
  });
});
