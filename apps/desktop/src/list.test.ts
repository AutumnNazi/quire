import { describe, expect, it } from "vitest";
import { groupByWeek, isUnread, isoWeekKey, selectClips, type ClipLike } from "./list";

function clip(partial: Partial<ClipLike> & { clippedAt: string }): ClipLike {
  return { title: "t", read: false, archived: false, ...partial };
}

describe("未读判定", () => {
  it("没读过、没归档的才算未读", () => {
    expect(isUnread(clip({ clippedAt: "2026-03-02T09:00:00+08:00" }))).toBe(true);
  });

  it("读过的不算未读", () => {
    expect(isUnread(clip({ clippedAt: "2026-03-02T09:00:00+08:00", read: true }))).toBe(false);
  });

  it("归档过的一律不算未读", () => {
    // 不管读没读过。用户归档了就是「这篇我处理完了」,还挂在未读队列里
    // 只会让「全部读完」这个念头永远达不成。
    expect(isUnread(clip({ clippedAt: "2026-03-02T09:00:00+08:00", archived: true }))).toBe(false);
    expect(
      isUnread(clip({ clippedAt: "2026-03-02T09:00:00+08:00", read: true, archived: true })),
    ).toBe(false);
  });
});

describe("ISO 周边界", () => {
  it("元旦那天归到本年第 1 周,不是去年年底", () => {
    // 这条是整个周回顾最容易错的地方。2026-01-01 是周四,按 ISO 规则
    // 属于 2026 年第 1 周;按日历年归就会掉进 2025 年。
    expect(isoWeekKey(new Date("2026-01-01T09:00:00+08:00"))).toEqual({
      isoYear: 2026,
      isoWeek: 1,
    });
  });

  it("元旦和同一周的周日归到同一周", () => {
    const thu = isoWeekKey(new Date("2026-01-01T09:00:00+08:00"));
    const sun = isoWeekKey(new Date("2026-01-04T20:00:00+08:00"));
    expect(sun).toEqual(thu);
  });

  it("周日晚上仍然算本周,不是下周", () => {
    // 周一到周日是一周。周日 23 点剪的东西归本周,不能因为过了午夜就跳走。
    const sundayNight = isoWeekKey(new Date("2026-01-04T23:30:00+08:00"));
    const monday = isoWeekKey(new Date("2026-01-05T00:30:00+08:00"));
    expect(sundayNight.isoWeek).toBe(1);
    expect(monday.isoWeek).toBe(2);
  });

  it("一年里的第一周和最后一周都不会算成 0 或 54", () => {
    for (const iso of ["2026-01-01", "2026-12-28", "2026-12-31", "2024-12-30"]) {
      const { isoWeek } = isoWeekKey(new Date(`${iso}T12:00:00+08:00`));
      expect(isoWeek).toBeGreaterThanOrEqual(1);
      expect(isoWeek).toBeLessThanOrEqual(53);
    }
  });
});

describe("按周分组", () => {
  it("最近的周排在最前", () => {
    const groups = groupByWeek([
      clip({ clippedAt: "2026-03-02T09:00:00+08:00" }),
      clip({ clippedAt: "2026-03-16T09:00:00+08:00" }),
      clip({ clippedAt: "2026-03-09T09:00:00+08:00" }),
    ]);
    // 2026-03-02 是第 12 周(不是第 2 周),三周分别是 12/11/10
    expect(groups.map((g) => g.isoWeek)).toEqual([12, 11, 10]);
  });

  it("同一周的剪藏落进同一组", () => {
    const groups = groupByWeek([
      clip({ clippedAt: "2026-03-02T09:00:00+08:00" }),
      clip({ clippedAt: "2026-03-06T09:00:00+08:00" }),
      clip({ clippedAt: "2026-03-08T09:00:00+08:00" }),
    ]);
    expect(groups).toHaveLength(1);
    expect(groups[0].clips).toHaveLength(3);
  });

  it("时间解析不了的剪藏不会让整个列表炸掉", () => {
    // 静默失败是高优缺陷:一条坏数据不该让用户打不开软件。
    const groups = groupByWeek([
      clip({ clippedAt: "不是时间" }),
      clip({ clippedAt: "2026-03-02T09:00:00+08:00" }),
    ]);
    expect(groups.length).toBeGreaterThan(0);
  });
});

describe("筛选与排序", () => {
  const all = [
    clip({ title: "a", clippedAt: "2026-03-02T09:00:00+08:00" }),
    clip({ title: "b", clippedAt: "2026-03-05T09:00:00+08:00", read: true }),
    clip({ title: "c", clippedAt: "2026-03-01T09:00:00+08:00", archived: true }),
    clip({ title: "d", clippedAt: "2026-03-06T09:00:00+08:00" }),
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
