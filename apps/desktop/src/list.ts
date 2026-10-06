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
  starred: boolean;
  /** 列表改的是同一个对象的副本,靠它认人。 */
  filename?: string;
}

/** 列表的显示模式。
 *
 *  `week` 不是「筛掉一部分」,而是换个排布方式,所以它和 all/unread 不是
 *  同一类东西——`selectClips` 不接它,只由 `groupByWeek` 处理。放进同一个
 *  联合类型里,调用方就得在每个 switch 里补一个永远走不到的分支。 */
export type ListFilter = "all" | "today" | "unread" | "archived" | "starred";
export type ListMode = ListFilter | "week";

/** 一周的分组。ISO 周只在这一处实现——后端曾经也有一份 `weekly_digest`,
 *  结果两边按不同的时区算周,同一篇剪藏在用户换时区后会落到不同的栏里。
 *  分组是纯展示逻辑,放在前端就够了,不值得为它多一次 IPC 往返。 */
export interface WeekGroup<T> {
  isoYear: number;
  isoWeek: number;
  clips: T[];
}

/** 未读队列:归档过的一律不算,不然「全部读完」这个念头永远达不成。
 *
 *  参数收的是**它真正用到的两项**,不是整个 `ClipLike`。统计那边
 *  (`stats.ts`)手里的篇没有标题也不需要标题——为了能调这个函数
 *  而在 `StatsClip` 里挂一个用不上的 `title`,那是让类型去迁就函数 */
export function isUnread(clip: { read: boolean; archived: boolean }): boolean {
  return !clip.read && !clip.archived;
}

/** 还欠着多少篇。
 *
 *  **界面上那个数字走这里,不走 `clips.filter(...)`。** 筛选列表和按钮上的
 *  数字各写一份判定的话,改了一处另一处没跟上,用户看到的「未读 8」点进去
 *  却是 11 篇——那比没有数字更糟,他会以为这软件算不准 */
export function unreadCount(clips: ClipLike[]): number {
  return clips.filter(isUnread).length;
}

/** 「今天」按**本地日历日**算,不是"24 小时内"。
 *
 *  **差在凌晨那一条。** 晚上 11 点存的一篇,凌晨 1 点看一眼,按"24 小时内"
 *  它还在今天,按日历日它已经算昨天了——而用户凌晨剪藏时想的是"我刚存的那篇"。
 *  更要紧的是反过来:早上 9 点存的一篇,晚上 11 点看,按 24 小时内还在,
 *  可用户会觉得"这都半天前了,还算今天?"
 *
 *  `now` 由调用方传进来:这个函数在测试里要能钉住"今天是哪天",
 *  直接调 `new Date()` 的话测试没法复现边界 */
export function isToday(clip: ClipLike, now: Date): boolean {
  const t = new Date(clip.clippedAt);
  // **解析不了就不算今天。** 一篇日期坏掉的剪藏,宁可从"今天"里漏掉,
  // 也不要因为 `NaN` 的比较结果是 false 而被塞进任何一天
  // 后面三个比较对 NaN 一律是 false,所以这一行**删掉结果也一样**。
  // 留着是因为它把"为什么返回 false"写成了显式的,而不是依赖
  // `NaN != NaN` 这个巧合——将来谁把比较换成 `==` 或加一个宽松分支,
  // 这里就是那道拦住他的东西
  if (Number.isNaN(t.getTime())) return false;
  return (
    t.getFullYear() === now.getFullYear() &&
    t.getMonth() === now.getMonth() &&
    t.getDate() === now.getDate()
  );
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
  // 收藏是**跨状态**的:收藏了的照样能同时是"未读"。
  // 所以它不能像归档那样"挪到一边",而是"挑出来看"——
  // 用户存到几百篇之后,真正的问题不是找不到,是"哪几篇值得再看一遍找不到"
  // 「今天」的基准时间**每次调用现取**。取一次传给 selectClips 看着更省,
  // 但那得让签名多一个参数,而每个调用点都要决定传什么——
  // 忘传的那个编译器不管,界面上就是一片空
  const now = new Date();
  const kept =
    filter === "today" ? clips.filter((c) => isToday(c, now))
    : filter === "unread" ? clips.filter(isUnread)
    : filter === "archived" ? clips.filter((c) => c.archived)
    : filter === "starred" ? clips.filter((c) => c.starred)
    : clips;
  return [...kept].sort(byClippedAtDesc);
}

/** 把一篇剪藏插进列表,同名的替换掉,并保持时间倒序。
 *
 *  撤销删除会往列表里放回一篇**旧**剪藏,直接 `clips = [back, ...clips]`
 *  会把它顶到最前面——顺序该由剪藏时间决定,不由用户刚才按了哪个键决定。 */
export function upsertClip<T extends ClipLike>(clips: T[], clip: T): T[] {
  const rest = clips.filter((c) => c.filename !== clip.filename);
  return [...rest, clip].sort(byClippedAtDesc);
}

/** 键盘上下移动选中项,返回新的选中文件名。列表空了就返回 null。
 *
 *  **停在两头,不循环。** 到底之后再按一下却弹回顶上,读着读着就被甩回
 *  第一篇,人会以为软件抽风。
 *
 *  `current` 不在 `visible` 里只是个兜底;「刚标完已读、接着读下一篇」
 *  那条正常路径走 `reanchorAfterRemoval`,它才知道该落到哪一篇。 */
export function moveSelection(visible: string[], current: string | null, delta: number): string | null {
  if (visible.length === 0) return null;
  const at = current === null ? -1 : visible.indexOf(current);
  if (at < 0) return delta > 0 ? visible[0] : visible[visible.length - 1];
  const next = at + delta;
  if (next < 0 || next >= visible.length) return visible[at];
  return visible[next];
}

/** 刚处理完一篇(标了已读、归档了)之后,选中项落到哪。
 *
 *  那一篇会从列表里消失,光按文件名已经找不到它的位置了。**得用下标**:
 *  停在 [a,b,c] 的 b 上标了已读,剩 [a,c],接着读的应该是 c——也就是原来
 *  它下面那一篇。要是图省事按名字找不到就回顶上,用户会以为自己刚读过
 *  的那篇又回来了,平白多读一遍。 */
export function reanchorAfterRemoval(visible: string[], indexBefore: number): string | null {
  if (visible.length === 0) return null;
  return visible[Math.min(Math.max(indexBefore, 0), visible.length - 1)];
}

/* ── 多选 ──
 *
 * 规则单独抽出来测,是因为这几条全是"用户以为选中了、其实没选中"那一类:
 * 选错了不报错,只是结果不对,点界面根本验不出来。 */

export type Selection = ReadonlySet<string>;

/** 加选/减选。返回新集合,不改原的——渲染会重算好几次,就地改容易漏。 */
export function toggleSelected(selected: Selection, filename: string): Set<string> {
  const next = new Set(selected);
  if (next.has(filename)) next.delete(filename);
  else next.add(filename);
  return next;
}

/** `Shift` 连选:从锚点选到当前这条,**两头都含**。
 *
 *  没有锚点就退化成"只选当前这条"——总比什么都不选强。 */
export function selectRange(
  visible: string[],
  anchor: string | null,
  target: string,
  base: Selection,
): Set<string> {
  const from = anchor === null ? -1 : visible.indexOf(anchor);
  const to = visible.indexOf(target);
  if (from === -1 || to === -1) return new Set([...base, target]);
  const [lo, hi] = from <= to ? [from, to] : [to, from];
  const next = new Set(base);
  // 从头重选这一段:Windows 资源管理器就是这个语义——连选是把区间**替换**掉,
  // 追加会攒出一堆用户早就忘了自己选过的东西
  for (let i = 0; i < from; i++) next.delete(visible[i]);
  for (let i = to + 1; i < visible.length; i++) next.delete(visible[i]);
  for (let i = lo; i <= hi; i++) next.add(visible[i]);
  return next;
}

/** 全选当前可见的。看不见的不选——用户看不到的东西被"选中"之后
 *  跟着一起删掉,那是制造事故,不是提高效率。 */
export function selectAll(visible: string[]): Set<string> {
  return new Set(visible);
}

/** 已经不在列表里的选中项顺手清掉。列表一变(筛选、删除、撤销)就得走一遍,
 *  否则用户会对着一堆看不见的条目按删除。 */
export function pruneSelection(selected: Selection, visible: string[]): Set<string> {
  const onScreen = new Set(visible);
  const next = new Set<string>();
  for (const filename of selected) {
    if (onScreen.has(filename)) next.add(filename);
  }
  return next;
}

/** 标签筛选。**正交于 `selectClips`**——能叠着用:「未读」里只看
 * 「待读」是最常见的用法,两个筛子各管各的。
 *
 *  `tag` 为 `null` 表示没筛,原样返回。**别把空串也当成"没有标签"**:
 *  空串是用户真敲进去过的标签名,和"不筛"是两回事。 */
export function selectByTag<T extends { tags: string[] }>(clips: T[], tag: string | null): T[] {
  if (tag === null) return clips;
  return clips.filter((c) => c.tags.includes(tag));
}

/** 界面上敲的标签入库存之前先洗一遍。跟后端 `clean_tags` 同一套规矩,
 *  但前端也得洗一遍:芯片要立刻长得对,不用等一次 IPC 往返回来才变。
 *
 *  **重复的不加。** 加两次同一个标签,标签栏上会出现两个「待读」,
 *  用户点其中一个只看到一半的文章,还以为是软件按标签筛错了。 */
export function withTag(current: string[], tag: string): string[] {
  const trimmed = tag.trim();
  if (!trimmed || current.includes(trimmed)) return current;
  return [...current, trimmed];
}

/** 摘掉一个标签。**一个标签只摘一次**:重名摘光的话,用户只是想改个名,
 *  结果标签整个没了。 */
export function withoutTag(current: string[], tag: string): string[] {
  return current.filter((t) => t !== tag);
}

/* ── 一屏画多少 ── */

/**
 * 一次往列表里塞多少条。
 *
 * **别拿它当业务规则,它是画布的容量。** 列表每一条都是十来个 DOM 节点
 * 加几个事件监听,库里上千篇的时候一次全画出来,滚一下就是几千个节点
 * 一起参与布局。用户存到几百篇正是这个软件**成功**的样子,不该在那一刻
 * 变卡
 *
 * 200 这个数不是拍脑袋:一屏大约能扫 15–20 条,200 条够翻十几屏,
 * 而节点总数压在两千上下——这是任何浏览器都能随手重排的量
 */
export const LIST_PAGE = 200;

/** 一屏要画哪些、后面还压着多少。
 *
 *  **`limit` 是"最多画几条",不是一个页码。** 页码语义下点「第 3 页」得先
 *  跳走再跳回来,而用户点「显示更多」想的是"接着往下看";而且往回翻时
 *  得判断当前在第几页,判断错了列表就跳一下
 *
 *  切筛选、切标签、搜索之后**都得把 limit 拨回 `LIST_PAGE`**。忘了拨的话,
 *  用户在上一个视图里点过两次「显示更多」,切过去第一眼看到的就是
 *  一个已经翻了两屏的列表——而他根本还没翻过
 */
export function listWindow<T>(items: T[], limit: number): { shown: T[]; hidden: number } {
  // 负数当没点过。**不是防御性编程,是防一个真会出现的值**:
  // 调用方拿 `limit - perPage` 往回退时,很容易退回负数
  const want = Math.max(0, limit);
  const shown = items.slice(0, want);
  return { shown, hidden: items.length - shown.length };
}

