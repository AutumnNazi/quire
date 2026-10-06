import { isUnread } from "./list";
import type { TagCount } from "./types";

/**
 * 「这库里有什么」。
 *
 * 存了一个月之后,真正的问题不是"某篇文章在哪儿"——是**这件事到底有没有
 * 在做**。用户存了九十篇、读完六十篇,和存了九十篇、一篇没读,是两个完全
 * 不同的处境,而列表页对这两个处境显示的是同一屏东西
 *
 * **算的是 `clips` 里那批,不是重扫磁盘。** 列表本来就把全库带回前端了
 * (`list_clips`),再让后端扫一遍的话,两份数会有一瞬间对不上——
 * 而统计页和列表页对不上的时候,用户信谁都不对
 *
 * 全是纯函数,不碰 DOM:`main.ts` 一 import 就 boot,逻辑留在那儿就只能
 * 靠点界面验证,而"这一篇到底算不算这个月的"这种边界正是点不出来的
 */

export interface SiteCount {
  /** 站点名。空串表示这篇没记站点(手工放进去的笔记多半是这样)。 */
  site: string;
  count: number;
}

export interface MonthCount {
  /** `YYYY-MM`,按**本地日历月**算。 */
  month: string;
  count: number;
}export interface LibraryStats {
  total: number;
  unread: number;
  archived: number;
  starred: number;
  /** 写了批注的。批注是自己花过时间的证据,是"这库有用"最实在的指标 */
  withNote: number;
  /** 读过一半又撂下的。**这个数字是给人看得心里痒的**——
   *  它正是"回去把它读完"的入口 */
  halfRead: number;
  bySite: SiteCount[];
  /** 最近 `MONTHS_SHOWN` 个月,**按时间顺序(旧的在先)**。
   *  空月也保留——见 `byMonth` 那段的注释。 */
  byMonth: MonthCount[];
  byTag: TagCount[];
}

/** 统计要看的那些篇的最小形状。比 `ClipSummary` 窄,方便测。 */
export interface StatsClip {
  site: string;
  clippedAt: string;
  read: boolean;
  archived: boolean;
  starred: boolean;
  progress: number;
  tags: string[];
  note: string;
}

/** 连续几个月的柱子。一年够看出趋势了,再多一屏也放不下。 */
export const MONTHS_SHOWN = 12;

/** 按月往回数 `count` 个月,返回 `YYYY-MM`,**新的在前**。 */
function monthKeys(now: Date, count: number): string[] {
  const out: string[] = [];
  for (let i = 0; i < count; i += 1) {
    const d = new Date(now.getFullYear(), now.getMonth() - i, 1);
    out.push(`${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`);
  }
  return out;
}

/** 一篇剪藏落在哪个月(本地日历月)。日期坏了就返回 null。 */
function monthOf(iso: string): string | null {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return null;
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`;
}

/** 篇数从多到少,一样多时按名字排。
 *
 *  **必须有一个确定的次序。** 只按 count 排的话,并列的那些每次渲染
 *  顺序都可能变——用户点开统计页,看见站点那一栏自己在跳 */
function byCountDesc<T extends { count: number }>(
  items: T[],
  key: (item: T) => string,
): T[] {
  return [...items].sort((a, b) => b.count - a.count || key(a).localeCompare(key(b)));
}

export function libraryStats(clips: StatsClip[], now = new Date()): LibraryStats {
  const siteCounts = new Map<string, number>();
  const tagCounts = new Map<string, number>();
  const monthCounts = new Map<string, number>();
  const months = monthKeys(now, MONTHS_SHOWN);

  let unread = 0;
  let archived = 0;
  let starred = 0;
  let withNote = 0;
  let halfRead = 0;

  for (const clip of clips) {
    if (isUnread(clip)) unread += 1;
    if (clip.archived) archived += 1;
    if (clip.starred) starred += 1;
    if (clip.note.trim() !== "") withNote += 1;
    // 归档的不算"读了一半撂下"——归档就是"这事儿翻篇了"
    if (clip.progress > 0 && !clip.read && !clip.archived) halfRead += 1;

    siteCounts.set(clip.site, (siteCounts.get(clip.site) ?? 0) + 1);
    for (const tag of clip.tags) tagCounts.set(tag, (tagCounts.get(tag) ?? 0) + 1);

    // **老剪藏不进趋势,靠的是下面那行只按 `months` 铺轴。**
    // 这里原本还有一句 `months.includes(month)` 的过滤,后来发现它
    // **不可观测**——Map 里多存一个键,`byMonth` 也永远读不到它。
    // 变异验证时那一处改坏了测试照样绿,才看出来它守的是个空门
    const month = monthOf(clip.clippedAt);
    if (month !== null) monthCounts.set(month, (monthCounts.get(month) ?? 0) + 1);
  }

  return {
    total: clips.length,
    unread,
    archived,
    starred,
    withNote,
    halfRead,
    bySite: byCountDesc(
      [...siteCounts].map(([site, count]) => ({ site, count })),
      (s) => s.site,
    ),
    // **空月也留着,补零。** 跳掉没有剪藏的月份,柱子会连成一排,
    // 而"三月一篇没存"恰恰是用户最该看见的那件事——
    // 那说明他三月很忙,或者他三月不用这个软件
    byMonth: months
      .map((month) => ({ month, count: monthCounts.get(month) ?? 0 }))
      .reverse(),
    byTag: byCountDesc(
      [...tagCounts].map(([tag, count]) => ({ tag, count })),
      (t) => t.tag,
    ),
  };
}
