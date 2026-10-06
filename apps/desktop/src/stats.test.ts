import { describe, expect, it } from "vitest";
import { MONTHS_SHOWN, libraryStats } from "./stats";
import type { StatsClip } from "./stats";

/**
 * 「这库里有什么」。
 *
 * 这些数字是**给用户看他自己**的,算错的后果比一个功能坏了更隐蔽:
 * 数少了,他会以为东西丢了(而这个软件的立身之本就是"东西都在");
 * 数多了,他会觉得自己比实际更勤快,然后不再信这一页
 *
 * 边界全在这儿钉住:跨年、空月、坏日期、空站点。
 */

function clip(over: Partial<StatsClip> = {}): StatsClip {
  return {
    site: "example.com",
    clippedAt: "2026-03-15T10:00:00+08:00",
    read: false,
    archived: false,
    starred: false,
    progress: 0,
    tags: [],
    note: "",
    ...over,
  };
}

/** 钉住"现在是哪一刻"。不钉的话,测试跑到明年就红了 */
const NOW = new Date(2026, 2, 20, 12, 0, 0); // 2026-03-20 本地时间

describe("总览", () => {
  it("空的库全是零,不报错", () => {
    const s = libraryStats([], NOW);
    expect(s.total).toBe(0);
    expect(s.unread).toBe(0);
    expect(s.bySite).toEqual([]);
    expect(s.byTag).toEqual([]);
  });

  it("未读只数没读、也没归档的", () => {
    // 归档过的不算未读,不然「全部读完」这个念头永远达不成。
    // 判定走 `list.ts` 里那一个,不在统计里再写一遍
    const s = libraryStats(
      [
        clip(),
        clip({ read: true }),
        clip({ archived: true }),
        clip({ read: true, archived: true }),
      ],
      NOW,
    );
    expect(s.total).toBe(4);
    expect(s.unread).toBe(1);
    expect(s.archived).toBe(2);
  });

  it("收藏、批注、读了一半各数各的", () => {
    const s = libraryStats(
      [
        clip({ starred: true }),
        clip({ note: "在这里写了点东西" }),
        clip({ note: "   " }), // 只有空白 = 没写
        clip({ progress: 0.4 }),
        // 读完的和归档的都不算"读了一半撂下"
        clip({ progress: 0.4, read: true }),
        clip({ progress: 0.4, archived: true }),
      ],
      NOW,
    );
    expect(s.starred).toBe(1);
    expect(s.withNote).toBe(1);
    expect(s.halfRead).toBe(1);
  });
});

describe("按站点", () => {
  it("数篇数,多的在前", () => {
    const s = libraryStats(
      [
        clip({ site: "a.com" }),
        clip({ site: "b.com" }),
        clip({ site: "a.com" }),
        clip({ site: "a.com" }),
        clip({ site: "b.com" }),
      ],
      NOW,
    );
    expect(s.bySite).toEqual([
      { site: "a.com", count: 3 },
      { site: "b.com", count: 2 },
    ]);
  });

  it("一样多的时候按名字排,不是随机", () => {
    // **没有第二排序键的话,并列的那些顺序会随引擎的实现变。**
    // 用户看到的是一栏站点自己在跳,而他会以为软件在乱读他的文件
    const s = libraryStats(
      [clip({ site: "z.com" }), clip({ site: "a.com" }), clip({ site: "m.com" })],
      NOW,
    );
    expect(s.bySite.map((x) => x.site)).toEqual(["a.com", "m.com", "z.com"]);
  });

  it("没记站点的也算一档,不悄悄丢掉", () => {
    // 手工放进来的笔记常常没有站点。**丢掉的话总数对不上**——
    // 用户加一下上面几栏,发现比总数少,只会以为统计坏了
    const s = libraryStats([clip({ site: "" }), clip({ site: "a.com" })], NOW);
    const sum = s.bySite.reduce((n, x) => n + x.count, 0);
    expect(sum).toBe(s.total);
  });
});

describe("按月", () => {
  it("返回固定的几个月,空月补零", () => {
    // **空月不能跳。** 跳掉没有剪藏的月份,柱子会连成一排,
    // 而"三月一篇没存"恰恰是用户最该看见的那件事
    const s = libraryStats([clip({ clippedAt: "2026-03-15T10:00:00+08:00" })], NOW);
    expect(s.byMonth).toHaveLength(MONTHS_SHOWN);
    // **时间轴从左到右是旧的新。** 倒过来的话柱子读起来是"越来越差",
    // 而用户这个月明明比上个月存得多
    expect(s.byMonth[MONTHS_SHOWN - 1]).toEqual({ month: "2026-03", count: 1 });
    expect(s.byMonth[MONTHS_SHOWN - 2]).toEqual({ month: "2026-02", count: 0 });
    expect(s.byMonth[0].month).toBe("2025-04");
  });

  it("跨年往回数也对", () => {
    // 一月往回数十二个月要跨到前年二月。算错的话柱子会少一根,
    // 而少的那根最旧,一般没人发现——直到有人真想比一年的趋势
    const s = libraryStats([], new Date(2026, 0, 5)); // 2026-01
    expect(s.byMonth[MONTHS_SHOWN - 1].month).toBe("2026-01");
    expect(s.byMonth[MONTHS_SHOWN - 2].month).toBe("2025-12");
    expect(s.byMonth[0].month).toBe("2025-02");
  });

  it("一年以外的剪藏不塞进趋势里", () => {
    // 几年前的剪藏算进去,那根柱子会高得离谱,而它说的不是
    // "你这个月存了很多"——那根柱子会永远压着别的
    //
    // **这条测的是结果,不是那句过滤。** 早些时候代码里有一句
    // `months.includes(month)` 做这个过滤,但那句是空的:轴是按
    // `months` 铺的,Map 里多存一个键根本读不到。变异验证才发现
    // 改坏它测试照样绿——所以那句删了,这条断言留下
    const s = libraryStats([clip({ clippedAt: "2019-03-15T10:00:00+08:00" })], NOW);
    expect(s.byMonth.every((m) => m.count === 0)).toBe(true);
    expect(s.byMonth.map((m) => m.month)).not.toContain("2019-03");
    // 但它照样算在总数里
    expect(s.total).toBe(1);
  });

  it("日期坏掉的篇不炸,也不算进趋势", () => {
    const s = libraryStats([clip({ clippedAt: "不是日期" })], NOW);
    expect(s.total).toBe(1);
    expect(s.byMonth.every((m) => m.count === 0)).toBe(true);
  });

  it("按月数的是本地日历月,不是 UTC", () => {
    // **这个月的第一天最容易露馅。** 东八区的 3 月 1 日凌晨,在 UTC 里
    // 还是 2 月 28 日——按 UTC 分的话它会掉进上个月的柱子
    //
    // **时间戳从本地时间构造,不写死偏移。** 写死 `+08:00` 的话,
    // 测试在别的时区跑就会红,而它验证的恰恰是"跟着本地走"
    const justAfterMidnight = new Date(2026, 2, 1, 0, 30);
    const s = libraryStats([clip({ clippedAt: justAfterMidnight.toISOString() })], NOW);
    expect(s.byMonth[MONTHS_SHOWN - 1]).toEqual({ month: "2026-03", count: 1 });
  });
});

describe("按标签", () => {
  it("一篇挂多个标签就各算一次", () => {
    const s = libraryStats(
      [clip({ tags: ["待读", "长文"] }), clip({ tags: ["待读"] })],
      NOW,
    );
    expect(s.byTag).toEqual([
      { tag: "待读", count: 2 },
      { tag: "长文", count: 1 },
    ]);
  });

  it("没有标签就是空的一栏,不是一栏空字符串", () => {
    // 空串会变成一个没有名字的标签,点上去筛出全库——
    // 用户看着一个空白标签能筛出所有东西,只会觉得标签功能坏了
    const s = libraryStats([clip({ tags: [] })], NOW);
    expect(s.byTag).toEqual([]);
  });
});
