import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * 批量撤销接线验得对不对。
 *
 * `batch-undo.test.ts` 测的是算法,这里测的是**接上去没有**。
 *
 * 要挡的回归很具体:算法全对、两处批量操作也都调了它,但**有一处忘了
 * 在动手之前拍快照**——那一处就会给用户一个撤销按钮,点下去把他
 * 改完的值又写回一遍(因为快照取的是改完之后的),界面还会说"已撤销"。
 * 运行时不报错,看着一切正常,只有用户某天发现"撤销之后还是改过的样子"
 */
describe("批量撤销接上了没有", () => {
  const mainSrc = readFileSync(resolve(process.cwd(), "src/main.ts"), "utf8");

  const calls = (name: string): number => mainSrc.split(name).length - 1;

  it("两处批量改动都在动手之前拍快照", () => {
    // **"之前"是判据。** 拍在 `await` 之后的话,快照里已经是改完的值,
    // 撤销就等于把他改的东西再写一遍
    const snap = calls("snapshotClips(clips, names,");
    expect(snap, "打标签和标标志两处都得拍").toBe(2);
    for (const [where, anchor] of [
      ["打标签", "const snaps = snapshotClips(clips, names, { tags: true });"],
      ["标标志", "const snaps = snapshotClips(clips, names, { read, archived, starred });"],
    ] as const) {
      expect(mainSrc, `${where}那处拍快照的写法得对`).toContain(anchor);
    }
  });

  it("快照拍在写盘之前,不是之后", () => {
    // 判据是**位置**,不是"有这行"。两处都查
    for (const fn of ["async function applyBatchTags()", "async function batchFlags("]) {
      const at = mainSrc.indexOf(fn);
      expect(at, `找得到 ${fn}`).toBeGreaterThan(-1);
      const block = mainSrc.slice(at, at + 1800);
      const shot = block.indexOf("snapshotClips(");
      const write = block.indexOf("await api.setClip");
      expect(shot, `${fn} 里得有拍快照`).toBeGreaterThan(-1);
      expect(write, `${fn} 里得有写盘`).toBeGreaterThan(-1);
      expect(shot, `${fn}:快照得拍在写盘之前`).toBeLessThan(write);
    }
  });

  it("撤销按钮只挂在有得撤销的时候", () => {
    // 一篇都没改成还弹撤销,用户点下去看到"已撤销"——他会以为这条路
    // 验过了,于是下次更敢批量操作
    const at = mainSrc.indexOf("function offerBatchUndo");
    expect(at, "得有 offerBatchUndo").toBeGreaterThan(-1);
    const fn = mainSrc.slice(at, at + 700);
    expect(fn, "得先判有没有可撤销的").toContain("snapshots.length === 0");
    expect(fn, "撤销按钮得在冒号后面那个分支里").toContain("toast.undo");
  });

  it("两处批量改动都把撤销摆出来了", () => {
    expect(calls("offerBatchUndo("), "两处调用 + 一处定义").toBe(3);
  });

  it("撤销是逐篇按旧值写,不是再翻一次", () => {
    // **这条是全部意义所在。** 撤销要是也发一次批量"全设成同一个值",
    // 那用户本来就标了已读的三篇会被翻成未读——撤销一次反而造出新错
    const at = mainSrc.indexOf("async function runBatchUndo");
    expect(at, "得有 runBatchUndo").toBeGreaterThan(-1);
    const fn = mainSrc.slice(at, at + 1800);
    expect(fn, "标签得逐篇发自己的那份").toContain("api.setClipTags(snap.filename, snap.tags");
    expect(fn, "标志得逐篇发自己的那份").toContain("api.setClipFlagsBatch(");
    // 一次批量设不了每篇不同的值,所以参数得是单篇数组
    expect(fn, "标志得一篇一条").toContain("[snap.filename],");
    // **不许退回批量**:那正是会造新错的那条路
    expect(fn, "不能拿整组去发批量").not.toContain("setClipFlagsBatch(names");
  });

  it("撤销失败会逐篇说清楚,不是笼统一句完事", () => {
    const at = mainSrc.indexOf("async function runBatchUndo");
    const fn = mainSrc.slice(at, at + 2200);
    expect(fn, "得记失败").toContain("toWireError(err)");
    expect(fn, "得报部分失败").toContain("error.undoPartial");
  });
});