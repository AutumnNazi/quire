import { describe, expect, it } from "vitest";
import { restorable, snapshotClips, undoBatch } from "./batch-undo";
import type { BatchReport, ClipSummary } from "./types";

function clip(over: Partial<ClipSummary> & { filename: string }): ClipSummary {
  return {
    id: over.filename,
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

function report(succeeded: string[], failed: string[] = []): BatchReport {
  return {
    succeeded,
    failed: failed.map((f) => ({ filename: f, reason: { code: "vault.io", args: {} } })),
  };
}

describe("批量撤销的快照", () => {
  it("只拍这次真要改的项", () => {
    // 「只标已读」把归档状态也拍下来的话,用户在中间手动归档过一篇,
    // 撤销就会拿这个旧值把它盖回去——凭空造出一个他没做过的改动
    const snaps = snapshotClips(
      [clip({ filename: "a.md", read: true, archived: true, starred: true })],
      ["a.md"],
      { read: false },
    );
    expect(snaps[0]).toEqual({ filename: "a.md", read: true });
  });

  it("这次没说要改的项,一项都不许进快照", () => {
    // **上一条传的是 `{ read: false }`,read 本来就在单子上**——
    // 所以"不管三七二十一全拍下来"的改法在它那儿是无差别的,照样绿。
    // 这条传的是「只归档」,快照里就该**只有** filename 和 archived:
    // 多出 read / starred 的话,撤销会拿这些旧值把用户中间手动改的盖掉
    const snaps = snapshotClips(
      [clip({ filename: "a.md", read: true, archived: false, starred: true })],
      ["a.md"],
      { archived: true },
    );
    expect(snaps[0]).toEqual({ filename: "a.md", archived: false });
  });

  it("快照里没有的项,写回去时也不许当成 false", () => {
    // `snapshotClips` 拍成什么样,`runBatchUndo` 就按什么样发出去。
    // **读出来是 `undefined` 和读出来是 `false` 在协议上是两件事**:
    // 前者是"别动这一项",后者是"把它改成 false"
    const snaps = snapshotClips([clip({ filename: "a.md", read: true })], ["a.md"], {
      archived: true,
    });
    expect("read" in snaps[0], "没说要改的项不该出现在快照里").toBe(false);
    expect("starred" in snaps[0], "没说要改的项不该出现在快照里").toBe(false);
  });

  it("标签是复制一份,不是同一个数组", () => {
    // 直接挂原数组的话,用户在撤销前又改了标签,快照跟着一起变,
    // 于是"撤销"撤到了一个从没存在过的状态
    const source = clip({ filename: "a.md", tags: ["旧"] });
    const snaps = snapshotClips([source], ["a.md"], { tags: true });
    source.tags.push("刚加的");
    expect(snaps[0].tags).toEqual(["旧"]);
  });

  it("选区外的篇不拍", () => {
    const snaps = snapshotClips(
      [clip({ filename: "a.md" }), clip({ filename: "b.md" })],
      ["a.md"],
      { read: false },
    );
    expect(snaps.map((s) => s.filename)).toEqual(["a.md"]);
  });
});

describe("哪些值得恢复", () => {
  it("没改成功的不恢复", () => {
    // 它的旧值原封不动还在文件里。写回去等于做了一次没必要的写盘,
    // 而"值没变就不写"是这库的一条规矩
    const snaps = snapshotClips(
      [clip({ filename: "a.md", read: true }), clip({ filename: "b.md", read: false })],
      ["a.md", "b.md"],
      { read: false },
    );
    const got = restorable(snaps, report(["a.md"], ["b.md"]));
    expect(got.map((s) => s.filename)).toEqual(["a.md"]);
  });

  it("全都没改成就没有可撤销的", () => {
    const snaps = snapshotClips([clip({ filename: "a.md" })], ["a.md"], { read: false });
    expect(restorable(snaps, report([], ["a.md"]))).toEqual([]);
  });
});

describe("撤销的执行", () => {
  it("标志按旧值写回去,不是再翻一次", () => {
    // **这条是全部意义所在。** 撤销要是也发一次 read=false,
    // 那用户本来就标了已读的三篇会被翻成未读——撤销一次反而造出新错
    const clips = [
      clip({ filename: "a.md", read: true }),
      clip({ filename: "b.md", read: true }),
    ];
    const snaps = snapshotClips(clips, ["a.md", "b.md"], { read: false });
    const sent: Array<Record<string, unknown>> = [];
    void undoBatch(snaps, async (group) => {
      sent.push({ read: group[0].read, n: group.length });
      return report(group.map((s) => s.filename));
    });
    expect(sent[0]).toEqual({ read: true, n: 2 });
  });

  it("标签那组单独发一条", () => {
    const snaps = snapshotClips([clip({ filename: "a.md", tags: ["甲"] })], ["a.md"], {
      tags: true,
    });
    const kinds: string[] = [];
    void undoBatch(snaps, async (group, kind) => {
      kinds.push(kind);
      expect(group[0].tags).toEqual(["甲"]);
      return report(group.map((s) => s.filename));
    });
    return expect(Promise.resolve(kinds)).resolves.toEqual(["tags"]);
  });

  it("一组炸了另一组照退,炸的那组逐篇记失败", () => {
    // 整组 reject(锁中毒)时**不能吞掉**。用户得知道这一组没退回来,
    // 而不是看见一句"撤销完成"
    const snaps = [
      { filename: "a.md", read: true },
      { filename: "b.md", tags: ["甲"] },
    ];
    const result = undoBatch(snaps, async (_group, kind) => {
      if (kind === "flags") throw new Error("锁中毒");
      return report(["b.md"]);
    });
    return expect(result).resolves.toEqual({
      restored: 1,
      failed: [{ filename: "a.md", reason: expect.any(Error) }],
    });
  });

  it("空快照什么都不发", () => {
    let called = false;
    return undoBatch([], async () => {
      called = true;
      return report([]);
    }).then(() => expect(called).toBe(false));
  });
});