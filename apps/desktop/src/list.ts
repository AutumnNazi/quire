/** 列表分组与筛选的纯逻辑,单独抽出来是为了能直接测。
 *
 *  这些规则是「每周回顾」和「未读队列」的语义所在:什么算未读、一周怎么分、
 *  筛选和搜索谁先谁后。一旦它们藏进 renderList 里,就只能靠点界面验证——
 *  而「点一下看对不对」测不出边界那一周。
 */

export interface ClipLike {
  title: string;
  clippedAt: string;
  read: boolean;
  archived: boolean;
}

/** 列表的显示模式。
 *
 *  `week` 不是「筛掉一部分」,而是换个排布方式,所以它和 all/unread 不是
 *  同一类东西——`selectClips` 不接它,只由 `groupByWeek` 处理。放进同一个
 *  联合类型里,调用方就得在每个 switch 里补一个永远走不到的分支。 */
export type ListFilter = "all" | "unread" | "archived";
export type ListMode = ListFilter | "week";

/** 一周的分组。用 ISO 周,和后端 `weekly_digest` 保持同一套规则。 */
export interface WeekGroup<T> {
  isoYear: number;
  isoWeek: number;
  clips: T[];
}

/** 未读队列:归档过的一律不算,不然「全部读完」这个念头永远达不成。 */
export function isUnread(clip: ClipLike): boolean {
  return !clip.read && !clip.archived;
}

/** 按剪藏时间倒序,新的在前。同毫秒时用标题兜底,保证顺序稳定。 */
export function byClippedAtDesc<T extends ClipLike>(a: T, b: T): number {
  if (a.clippedAt === b.clippedAt) return a.title.localeCompare(b.title, "zh-CN");
  return a.clippedAt < b.clippedAt ? 1 : -1;
}

/** ISO 8601 周序号:包含该周星期四的那一年才是归属年。
 *
 *  `getFullYear()` 直接拿日历年是错的——2026-01-01 是周四,它属于 2026 年第 1 周,
 *  按日历年归就会掉进 2025 年,用户元旦剪的东西会出现在去年年底那栏里。 */
export function isoWeekKey(date: Date): { isoYear: number; isoWeek: number } {
  const target = new Date(Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()));
  // 挪到本周四:day 0=周一 … 6=周日,+3 让周日落到同一天的星期四
  const dayNumber = (target.getUTCDay() + 6) % 7;
  target.setUTCDate(target.getUTCDate() - dayNumber + 3);
  const isoYear = target.getUTCFullYear();
  // 该年 1 月 4 日必定落在第 1 周
  const firstThursday = new Date(Date.UTC(isoYear, 0, 4));
  const firstDayNumber = (firstThursday.getUTCDay() + 6) % 7;
  firstThursday.setUTCDate(firstThursday.getUTCDate() - firstDayNumber + 3);
  const isoWeek = 1 + Math.round((target.getTime() - firstThursday.getTime()) / (7 * 86400000));
  return { isoYear, isoWeek };
}

/** 解析剪藏时间。存的是带时区的 ISO 字符串,解析不了就退到 epoch,
 *  宁可分错组也不能让整个列表炸掉——一条坏数据不该让用户打不开软件。 */
function parseWhen(iso: string): Date {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? new Date(0) : d;
}

/** 按 ISO 周分组,最近的周在最前。 */
export function groupByWeek<T extends ClipLike>(clips: T[]): WeekGroup<T>[] {
  const buckets = new Map<string, WeekGroup<T>>();
  for (const clip of clips) {
    const { isoYear, isoWeek } = isoWeekKey(parseWhen(clip.clippedAt));
    const key = `${isoYear}-W${isoWeek}`;
    const bucket = buckets.get(key);
    if (bucket) {
      bucket.clips.push(clip);
    } else {
      buckets.set(key, { isoYear, isoWeek, clips: [clip] });
    }
  }
  return [...buckets.values()].sort(
    (a, b) => b.isoYear - a.isoYear || b.isoWeek - a.isoWeek,
  );
}

/** 筛选 + 排序。搜索命中与否不在这里管——那是另一回事,别混进一个函数。 */
export function selectClips<T extends ClipLike>(clips: T[], filter: ListFilter): T[] {
  // 归档是「挪到一边」不是「删掉」,所以必须有个地方能翻回来。
  // 没有归档视图的话,用户点完归档东西就凭空消失了,那是数据丢失的观感。
  const kept =
    filter === "unread" ? clips.filter(isUnread)
    : filter === "archived" ? clips.filter((c) => c.archived)
    : clips;
  return [...kept].sort(byClippedAtDesc);
}
