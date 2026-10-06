import { describe, expect, it } from "vitest";
import {
  groupByWeek,
  isToday,
  isUnread,
  isoWeekKey,
  moveSelection,
  pruneSelection,
  reanchorAfterRemoval,
  selectAll,
  selectByTag,
  selectClips,
  selectRange,
  toggleSelected,
  unreadCount,
  upsertClip,
  listWindow,
  LIST_PAGE,
  withTag,
  withoutTag,
  type ClipLike,
} from "./list";

function clip(partial: Partial<ClipLike> & { clippedAt: string; filename?: string }): ClipLike {
  return { title: "t", read: false, archived: false, starred: false, filename: partial.filename ?? partial.title, ...partial };
}

describe("未读判定", () => {
  it("没读过、没归档的才算未读", () => {
    expect(isUnread(clip({ clippedAt: "2026-03-02T09:00:00" }))).toBe(true);
  });

  it("读过的不算未读", () => {
    expect(isUnread(clip({ clippedAt: "2026-03-02T09:00:00", read: true }))).toBe(false);
  });

  it("归档过的一律不算未读", () => {
    // 不管读没读过。用户归档了就是「这篇我处理完了」,还挂在未读队列里
    // 只会让「全部读完」这个念头永远达不成。
    expect(isUnread(clip({ clippedAt: "2026-03-02T09:00:00", archived: true }))).toBe(false);
    expect(
      isUnread(clip({ clippedAt: "2026-03-02T09:00:00", read: true, archived: true })),
    ).toBe(false);
  });
});

describe("ISO 周边界", () => {
  it("元旦那天归到本年第 1 周,不是去年年底", () => {
    // 这条是整个周回顾最容易错的地方。2026-01-01 是周四,按 ISO 规则
    // 属于 2026 年第 1 周;按日历年归就会掉进 2025 年。
    expect(isoWeekKey(new Date("2026-01-01T09:00:00"))).toEqual({
      isoYear: 2026,
      isoWeek: 1,
    });
  });

  it("元旦和同一周的周日归到同一周", () => {
    const thu = isoWeekKey(new Date("2026-01-01T09:00:00"));
    const sun = isoWeekKey(new Date("2026-01-04T20:00:00"));
    expect(sun).toEqual(thu);
  });

  it("周日晚上仍然算本周,不是下周", () => {
    // 周一到周日是一周。周日 23 点剪的东西归本周,不能因为过了午夜就跳走。
    //
    // **这里刻意不带时区偏移。** `isoWeekKey` 读的是 getFullYear/getDate 这些
    // 本地时间方法,所以它问的是"按**用户此刻所在时区**算,这是第几周"。
    // 早先这里写的是 "2026-01-05T00:30:00+08:00":在东八区是周一(第 2 周),
    // 在 UTC runner 上就折回周日(第 1 周)——本机绿、CI 红,查起来很费时间。
    // 不带偏移的字符串按本地时间解析,任何时区下断言都成立。
    const sundayNight = isoWeekKey(new Date("2026-01-04T23:30:00"));
    const monday = isoWeekKey(new Date("2026-01-05T00:30:00"));
    expect(sundayNight.isoWeek).toBe(1);
    expect(monday.isoWeek).toBe(2);
  });

  it("一年里的第一周和最后一周都不会算成 0 或 54", () => {
    for (const iso of ["2026-01-01", "2026-12-28", "2026-12-31", "2024-12-30"]) {
      const { isoWeek } = isoWeekKey(new Date(`${iso}T12:00:00`));
      expect(isoWeek).toBeGreaterThanOrEqual(1);
      expect(isoWeek).toBeLessThanOrEqual(53);
    }
  });
});

describe("按周分组", () => {
  it("最近的周排在最前", () => {
    const groups = groupByWeek([
      clip({ clippedAt: "2026-03-02T09:00:00" }),
      clip({ clippedAt: "2026-03-16T09:00:00" }),
      clip({ clippedAt: "2026-03-09T09:00:00" }),
    ]);
    // 2026-03-02 是第 12 周(不是第 2 周),三周分别是 12/11/10
    expect(groups.map((g) => g.isoWeek)).toEqual([12, 11, 10]);
  });

  it("同一周的剪藏落进同一组", () => {
    const groups = groupByWeek([
      clip({ clippedAt: "2026-03-02T09:00:00" }),
      clip({ clippedAt: "2026-03-06T09:00:00" }),
      clip({ clippedAt: "2026-03-08T09:00:00" }),
    ]);
    expect(groups).toHaveLength(1);
    expect(groups[0].clips).toHaveLength(3);
  });

  it("时间解析不了的剪藏不会让整个列表炸掉", () => {
    // 静默失败是高优缺陷:一条坏数据不该让用户打不开软件。
    const groups = groupByWeek([
      clip({ clippedAt: "不是时间" }),
      clip({ clippedAt: "2026-03-02T09:00:00" }),
    ]);
    expect(groups.length).toBeGreaterThan(0);
  });
});

describe("归档视图", () => {
  const mixed = [
    clip({ title: "未读", clippedAt: "2026-03-02T09:00:00" }),
    clip({ title: "已读", clippedAt: "2026-03-03T09:00:00", read: true }),
    clip({ title: "已归档", clippedAt: "2026-03-04T09:00:00", archived: true }),
    clip({ title: "又归档又读了", clippedAt: "2026-03-05T09:00:00", read: true, archived: true }),
  ];

  it("归档视图只给归档过的", () => {
    expect(selectClips(mixed, "archived").map((c) => c.title)).toEqual(["又归档又读了", "已归档"]);
  });

  it("归档和未读是两个互不包含的集合", () => {
    // 归档过的绝不能出现在未读里,否则「都读完了」这个念头永远达不成
    const unread = selectClips(mixed, "unread").map((c) => c.title);
    const archived = selectClips(mixed, "archived").map((c) => c.title);
    expect(unread).not.toContain("已归档");
    expect(archived).not.toContain("未读");
  });

  it("全部视图里归档过的仍在,不会凭空消失", () => {
    // 归档是「挪到一边」不是「删掉」。用户归档完回头找,必须在全部里还看得见。
    expect(selectClips(mixed, "all").map((c) => c.title)).toContain("已归档");
  });
});

describe("筛选与排序", () => {
  const all = [
    clip({ title: "a", clippedAt: "2026-03-02T09:00:00" }),
    clip({ title: "b", clippedAt: "2026-03-05T09:00:00", read: true }),
    clip({ title: "c", clippedAt: "2026-03-01T09:00:00", archived: true }),
    clip({ title: "d", clippedAt: "2026-03-06T09:00:00" }),
  ];

  it("全部视图按剪藏时间倒序", () => {
    expect(selectClips(all, "all").map((c) => c.title)).toEqual(["d", "b", "a", "c"]);
  });

  it("未读视图滤掉读过的和归档的", () => {
    expect(selectClips(all, "unread").map((c) => c.title)).toEqual(["d", "a"]);
  });

  it("筛选不会改坏原数组", () => {
    // 渲染前先筛选、渲染时又按别的顺序排——就地 sort 会让状态跟着变,
    // 切回去时列表顺序就莫名其妙了。
    const before = all.map((c) => c.title);
    selectClips(all, "unread");
    expect(all.map((c) => c.title)).toEqual(before);
  });
});

describe("插回列表", () => {
  const a = clip({ title: "3月3日", clippedAt: "2026-03-03T09:00:00" });
  const b = clip({ title: "3月1日", clippedAt: "2026-03-01T09:00:00" });
  const c = clip({ title: "3月2日", clippedAt: "2026-03-02T09:00:00" });

  it("放回来的是旧的也要回到它自己的位置上", () => {
    // 撤销的时候最容易写成 `clips = [back, ...clips]`,那样删除一篇旧的
    // 再撤销,它会窜到列表最前面。顺序是剪藏时间,不是操作顺序
    expect(upsertClip([a, c], b).map((x) => x.title)).toEqual(["3月3日", "3月2日", "3月1日"]);
  });

  it("已存在的按文件名替换,不会变成两条", () => {
    const changed = { ...b, title: "改过标题" };
    const out = upsertClip([a, b, c], changed);
    // 它还是 3月1日那篇,只换了标题,所以位置不变
    expect(out.map((x) => x.title)).toEqual(["3月3日", "3月2日", "改过标题"]);
    expect(out.filter((x) => x.filename === b.filename)).toHaveLength(1);
  });

  it("不改传进来的数组", () => {
    const before = [a, c];
    upsertClip(before, b);
    expect(before).toHaveLength(2);
  });
});

describe("键盘移动选中", () => {
  const list = ["a.md", "b.md", "c.md"];

  it("空列表就没有选中可言", () => {
    expect(moveSelection([], null, 1)).toBeNull();
    expect(moveSelection([], "a.md", -1)).toBeNull();
  });

  it("还没选中时,下键挑第一篇、上键挑最后一篇", () => {
    // 上键往回翻,人一般是从最新的开始往下读,不是从最老的
    expect(moveSelection(list, null, 1)).toBe("a.md");
    expect(moveSelection(list, null, -1)).toBe("c.md");
  });

  it("停在两头,不循环", () => {
    expect(moveSelection(list, "a.md", -1)).toBe("a.md");
    expect(moveSelection(list, "c.md", 1)).toBe("c.md");
  });

  it("中间就一步一格", () => {
    expect(moveSelection(list, "a.md", 1)).toBe("b.md");
    expect(moveSelection(list, "b.md", -1)).toBe("a.md");
  });

  it("选中的那篇不在列表里了,下键回顶上、上键回末尾", () => {
    // 这是兜底,不是正常路径。正常路径走 reanchorAfterRemoval
    expect(moveSelection(["b.md", "c.md"], "a.md", 1)).toBe("b.md");
    expect(moveSelection(["b.md", "c.md"], "a.md", -1)).toBe("c.md");
  });
});

describe("标完已读之后接着读", () => {
  it("下一篇顶上来,不用每篇都重新按一次下一条", () => {
    // 停在 b(下标 1),标了已读它就走了,剩 [a, c] —— 落点该是 c,
    // 也就是原来它下面那一篇,而不是退回 a 让用户重读一遍
    expect(reanchorAfterRemoval(["a.md", "c.md"], 1)).toBe("c.md");
  });

  it("删的是最后一篇,往前挪一位", () => {
    expect(reanchorAfterRemoval(["a.md", "b.md"], 2)).toBe("b.md");
  });

  it("全删光了就没有选中可言", () => {
    expect(reanchorAfterRemoval([], 0)).toBeNull();
  });

  it("没选中过就挑第一篇", () => {
    expect(reanchorAfterRemoval(["a.md", "b.md"], -1)).toBe("a.md");
  });
});

describe("多选", () => {
  const list = ["a", "b", "c", "d"];

  it("点一下选上,再点一下取消", () => {
    const once = toggleSelected(new Set(), "b");
    expect([...once]).toEqual(["b"]);
    expect([...toggleSelected(once, "b")]).toEqual([]);
  });

  it("不改动传进来的那个集合", () => {
    // 渲染会重算好几次,就地改的话总有一处会漏
    const base = new Set(["a"]);
    toggleSelected(base, "b");
    expect([...base]).toEqual(["a"]);
  });

  it("按住 Shift 连选,两头都算上", () => {
    const next = selectRange(list, "b", "d", new Set());
    expect([...next].sort()).toEqual(["b", "c", "d"]);
  });

  it("往回连选也是同一个区间", () => {
    const next = selectRange(list, "d", "b", new Set());
    expect([...next].sort()).toEqual(["b", "c", "d"]);
  });

  it("连选是把区间替换掉,不是往上加", () => {
    // 追加会攒出一堆用户早就忘了自己选过的东西
    const next = selectRange(list, "b", "c", new Set(["a"]));
    expect([...next].sort()).toEqual(["b", "c"]);
  });

  it("没有锚点时退化成只选当前这条", () => {
    const next = selectRange(list, null, "c", new Set(["a"]));
    expect([...next].sort()).toEqual(["a", "c"]);
  });

  it("锚点已经不在列表里了也不能整段乱选", () => {
    const next = selectRange(["b", "c", "d"], "z", "d", new Set());
    expect([...next]).toEqual(["d"]);
  });

  it("全选只选看得见的", () => {
    expect([...selectAll(list)]).toEqual(list);
  });

  it("列表一变就把看不见的选中项清掉", () => {
    // 用户对着一堆看不见的条目按删除,那是制造事故
    const next = pruneSelection(new Set(["a", "z"]), list);
    expect([...next]).toEqual(["a"]);
  });
});

describe("标签筛选", () => {
  const clips = [
    { filename: "a", tags: ["待读", "rust"] },
    { filename: "b", tags: ["已读"] },
    { filename: "c", tags: [] },
  ];

  it("没筛标签时原样返回", () => {
    expect(selectByTag(clips, null)).toHaveLength(3);
  });

  it("只留带那个标签的", () => {
    expect(selectByTag(clips, "待读").map((c) => c.filename)).toEqual(["a"]);
  });

  it("空标签数组的选不中", () => {
    // 空串是用户真敲进去过的标签名,不是"没有标签"。混为一谈的话
    // 用户起了个空标签就会把整库筛空,而他以为自己没在筛
    expect(selectByTag(clips, "")).toEqual([]);
  });

  it("没有的标签选出来是空的,不是全部", () => {
    expect(selectByTag(clips, "压根没有")).toEqual([]);
  });
});

describe("增删标签", () => {
  it("加上去", () => {
    expect(withTag(["a"], "b")).toEqual(["a", "b"]);
  });

  it("重复的不加,返回原数组", () => {
    const before = ["a"];
    expect(withTag(before, "a")).toBe(before);
  });

  it("空白的加不进去", () => {
    expect(withTag(["a"], "   ")).toEqual(["a"]);
    expect(withTag(["a"], "")).toEqual(["a"]);
  });

  it("首尾空白先去掉再存", () => {
    expect(withTag([], "  rust  ")).toEqual(["rust"]);
  });

  it("摘掉一个", () => {
    expect(withoutTag(["a", "b", "c"], "b")).toEqual(["a", "c"]);
  });

  it("摘不存在的标签不动它", () => {
    expect(withoutTag(["a"], "z")).toEqual(["a"]);
  });
});

describe("收藏筛选", () => {
  it("只挑出收藏了的", () => {
    const clips = [
      clip({ title: "甲", clippedAt: "2026-03-02T09:00:00", starred: true }),
      clip({ title: "乙", clippedAt: "2026-03-01T09:00:00" }),
    ];
    expect(selectClips(clips, "starred").map((c) => c.title)).toEqual(["甲"]);
  });

  it("收藏**不**影响是不是未读", () => {
    // 收藏是跨状态的:一篇可以既收藏、又还没读。
    // 如果收藏把它从"未读"里踢掉了,用户标星的瞬间这篇就从队列里消失了,
    // 那等于收藏顺手做了归档的事
    const starred = clip({ title: "甲", clippedAt: "2026-03-02T09:00:00", starred: true });
    expect(isUnread(starred)).toBe(true);
    expect(selectClips([starred], "unread").map((c) => c.title)).toEqual(["甲"]);
  });

  it("收藏也不影响归档", () => {
    // 归档是"读完挪走",收藏是"一直留着"。两个同时为真是合法的
    const both = clip({
      title: "甲",
      clippedAt: "2026-03-02T09:00:00",
      read: true,
      archived: true,
      starred: true,
    });
    expect(selectClips([both], "starred").map((c) => c.title)).toEqual(["甲"]);
    expect(selectClips([both], "archived").map((c) => c.title)).toEqual(["甲"]);
  });

  it("一个都没收藏时是空列表,不是全库", () => {
    const clips = [clip({ title: "甲", clippedAt: "2026-03-02T09:00:00" })];
    expect(selectClips(clips, "starred")).toEqual([]);
  });
});

describe("「今天」按日历日判定", () => {
  // **基准时间由测试给死。** 直接 `new Date()` 的话测试没法复现边界:
  // 跑在 23:59 和跑在 00:01 是两个结果,而 CI 什么时候跑是随机的
  const now = new Date("2026-03-05T12:00:00");

  it("今天早上剪的算今天", () => {
    expect(isToday(clip({ clippedAt: "2026-03-05T08:00:00" }), now)).toBe(true);
  });

  it("今天深夜剪的也算今天", () => {
    expect(isToday(clip({ clippedAt: "2026-03-05T23:30:00" }), now)).toBe(true);
  });

  it("昨天的不算", () => {
    expect(isToday(clip({ clippedAt: "2026-03-04T23:59:00" }), now)).toBe(false);
  });

  /** **凌晨那一篇归今天,不归昨天。** 晚上 11 点存的、凌晨 1 点看的,
   *  按「24 小时内」它已经过期了,可用户凌晨剪藏时想的是「我刚存的那篇」。
   *  这是选日历日而不是滚动 24 小时的主要理由 */
  it("凌晨剪的还归今天,不因为过了午夜就变成昨天", () => {
    const afterMidnight = new Date("2026-03-05T01:00:00");
    expect(isToday(clip({ clippedAt: "2026-03-05T01:00:00" }), afterMidnight)).toBe(true);
    expect(isToday(clip({ clippedAt: "2026-03-04T23:30:00" }), afterMidnight)).toBe(false);
  });

  /** 月份和年份都要比。** 只比「日」的话,去年的今天会出现在今年的今天里 */
  it("去年的同一天不算今天", () => {
    expect(isToday(clip({ clippedAt: "2025-03-05T08:00:00" }), now)).toBe(false);
  });

  it("上个月的不算", () => {
    expect(isToday(clip({ clippedAt: "2026-02-05T08:00:00" }), now)).toBe(false);
  });

  /** 日期坏掉的剪藏**宁可从「今天」里漏掉**。`NaN` 的比较一律是 false,
   *  塞进任何一天都是错的 */
  it("日期坏掉的不算今天", () => {
    expect(isToday(clip({ clippedAt: "压根不是日期" }), now)).toBe(false);
    expect(isToday(clip({ clippedAt: "" }), now)).toBe(false);
  });

  /** 空偏移(`Z`)按**本地时区**算。带偏移的按偏移算完再比本地日历日——
   *  「今天剪的」对用户永远是"他所在时区的今天" */
  it("带时区偏移的按换算后的本地时间算", () => {
    // UTC 的 2026-03-04T20:00 在 UTC+8 是 3 月 5 日凌晨 4 点
    expect(isToday(clip({ clippedAt: "2026-03-04T20:00:00Z" }), now)).toBe(
      new Date("2026-03-04T20:00:00Z").getDate() === 5,
    );
  });
});

/**
 * 工具栏「未读」按钮上那个数字。
 *
 * **它得和「未读」筛出来的条数一样。** 不一样的话用户点一下数字、
 * 看到的列表条数对不上,他会认为这软件算不准——那比不显示数字糟糕得多,
 * 因为一个不显示的数字不会引发任何信任问题
 */
describe("未读计数", () => {
  it("和「未读」筛出来的条数一致", () => {
    const clips = [
      clip({ clippedAt: "2026-03-01T09:00:00", title: "甲" }),
      clip({ clippedAt: "2026-03-01T09:00:00", title: "乙", read: true }),
      clip({ clippedAt: "2026-03-01T09:00:00", title: "丙", archived: true }),
      clip({ clippedAt: "2026-03-01T09:00:00", title: "丁" }),
    ];
    expect(unreadCount(clips)).toBe(selectClips(clips, "unread").length);
  });

  it("归档过的不算,读过的不算", () => {
    expect(
      unreadCount([
        clip({ clippedAt: "2026-03-01T09:00:00", title: "甲" }),
        clip({ clippedAt: "2026-03-01T09:00:00", title: "乙", read: true }),
        clip({ clippedAt: "2026-03-01T09:00:00", title: "丙", archived: true }),
      ]),
    ).toBe(1);
  });

  it("空库是 0", () => {
    expect(unreadCount([])).toBe(0);
  });
});


describe("一屏画多少", () => {
  const many = (n: number): number[] => Array.from({ length: n }, (_, i) => i);

  it("比一屏少的时候全画出来,不报还有", () => {
    // 库里一共就 12 篇,却挂着一条「还有 0 篇」,用户会以为下面还有东西
    const { shown, hidden } = listWindow(many(12), LIST_PAGE);
    expect(shown).toHaveLength(12);
    expect(hidden).toBe(0);
  });

  it("正好一屏也不报还有", () => {
    const { shown, hidden } = listWindow(many(LIST_PAGE), LIST_PAGE);
    expect(shown).toHaveLength(LIST_PAGE);
    expect(hidden).toBe(0);
  });

  it("超出一屏就截住,并且说得出还剩多少", () => {
    // **这是整个上限的立身之本。** 截住了却不说,用户存了八百篇
    // 只看到两百,会以为剩下的丢了
    const { shown, hidden } = listWindow(many(800), LIST_PAGE);
    expect(shown).toHaveLength(LIST_PAGE);
    expect(hidden).toBe(800 - LIST_PAGE);
  });

  it("放行更多之后接着往下画,从头画的不重样", () => {
    // 第二屏画的是 200..399,不是又把 0..199 画一遍——
    // 重样的话界面上会出现两遍同样的东西
    const { shown } = listWindow(many(800), LIST_PAGE * 2);
    expect(shown).toHaveLength(LIST_PAGE * 2);
    expect(shown[0]).toBe(0);
    expect(shown[LIST_PAGE * 2 - 1]).toBe(LIST_PAGE * 2 - 1);
  });

  it("放到全放完时不再报还有", () => {
    const { shown, hidden } = listWindow(many(300), 9999);
    expect(shown).toHaveLength(300);
    expect(hidden).toBe(0);
  });

  it("上限是 0 时不画,但压着的全都算数", () => {
    // 截断的回退路径。**不能说"画了 0 条、还剩 0 条"**——
    // 那是一个空白列表配一句"到底了",用户会以为库是空的
    const { shown, hidden } = listWindow(many(30), 0);
    expect(shown).toEqual([]);
    expect(hidden).toBe(30);
  });

  it("上限成了负数也不炸", () => {
    // 调用方拿 `limit - perPage` 往回退时很容易退成负数。
    // 负数传给 slice 会从屁股后面切——切出来的是**最后几条**,
    // 那比报错更难查:列表看着是满的,只是顺序全乱了
    const { shown, hidden } = listWindow(many(30), -5);
    expect(shown).toEqual([]);
    expect(hidden).toBe(30);
  });

  it("空库怎么切都是空的", () => {
    expect(listWindow([], LIST_PAGE)).toEqual({ shown: [], hidden: 0 });
    expect(listWindow([], 0)).toEqual({ shown: [], hidden: 0 });
  });
});
