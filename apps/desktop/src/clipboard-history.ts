/** 剪藏历史:弹出来过、但用户没当场决定的东西,事后还能找回来。
 *
 *  ## 为什么它存在
 *
 *  剪贴板监控弹出一条提示时,用户只有两个选择:存,或者忽略。**忽略之后
 *  什么都没留下**——他可能只是当时在忙,三分钟后想起来"刚才那个该存",
 *  却已经没地方找了。这是稍后读工具最常见的流失点:东西进不来,不是
 *  存不下,是错过之后就再也找不回来。
 *
 *  ## 为什么只放内存里
 *
 *  **这是一份会话内的临时状态,不是用户的数据。** 落盘会变成剪藏库之外的
 *  第二套数据源,而那份数据用户既不会去管理、也不会去删除——半年后剪藏库里
 *  多出几千条"曾经弹过但没存"的历史,那不是备份,那是垃圾。
 *
 *  真正存下来的东西永远在 `clips/` 下的 Markdown 里,那是唯一真相。
 *  这里存的是**指针之外的东西**:原样的一份,方便用户事后决定
 */

/** 一条历史。`html` 是原样的剪贴板 HTML,`text` 是纯文本版。
 *
 *  **两个都留着。** 扩展剪藏走 HTML 通道,只有 HTML 才带得上封面、作者
 *  这些字段;而用户手动复制一段话时只有文本。存一份就有一半的场景退化成
 *  "丢字段的剪藏" */
export interface HistoryItem {
  /** 唯一 id。用时间戳 + 序号,不用随机串——同一毫秒内可能弹两条 */
  id: string;
  text: string;
  html: string | null;
  url: string | null;
  /** 扩展塞进 HTML 的元数据(键形如 `quire-title`)。
   *
   *  **必须存。** 少了它,从历史补存的一篇会丢掉标题、封面、作者——
   *  用户明明是按扩展那条路剪的,补存之后却变成一篇没标题的裸文,
   *  而没有任何提示告诉他少了东西 */
  meta: Record<string, string>;
  /** 第几次弹出来的。同一段内容反复被复制,用户会反复看见同一条 */
  seen: number;
  firstSeen: number;
  lastSeen: number;
  saved: boolean;
}

/** 留多少条。**内存里的东西不能无限涨**——用户一整天开着软件,
 *  复制几百次是常事,不清掉的话到晚上就是几万条 */
const MAX = 200;

/** 内存表。**刻意不落盘**,理由见文件头 */
let items: HistoryItem[] = [];
let seq = 0;

function nextId(at: number): string {
  seq += 1;
  return `${at}-${seq}`;
}

export function record(input: {
  text: string;
  html: string | null;
  url: string | null;
  meta?: Record<string, string>;
  at: number;
}): HistoryItem {
  // 同一段内容再弹一次,**并进已有那条而不是新开一条**。
  // 新开的话用户会看到一长列一模一样的条目,抽屉直接失去意义
  const existing = items.find((i) => i.text === input.text && !i.saved);
  if (existing) {
    existing.seen += 1;
    existing.lastSeen = input.at;
    // 后一次带的 HTML 更全(扩展可能第二次才注入完),有新的就换上去
    if (input.html) existing.html = input.html;
    if (input.url) existing.url = input.url;
    // 后一次带的元数据更全就用它覆盖。**空的不覆盖**——
    // 扩展偶尔会先塞纯文本、后补 HTML,那时候后来的空 meta 会把
    // 之前那份好的抹掉
    if (input.meta && Object.keys(input.meta).length > 0) {
      existing.meta = input.meta;
    }
    return existing;
  }
  const item: HistoryItem = {
    id: nextId(input.at),
    text: input.text,
    html: input.html,
    url: input.url,
    meta: input.meta ?? {},
    seen: 1,
    firstSeen: input.at,
    lastSeen: input.at,
    saved: false,
  };
  items.unshift(item);
  if (items.length > MAX) items.length = MAX;
  return item;
}

export function markSaved(id: string): void {
  const item = items.find((i) => i.id === id);
  if (!item) return;
  // 存下来的**不删掉,标上**。用户存完又后悔想看看刚才那批里还有没有
  // 漏的,列表里标一个「已存」比直接消失更省事
  item.saved = true;
}

export function all(): HistoryItem[] {
  return items;
}

/** 用户可能想找回的:弹过但没存。**已存的不占位置**——
 *  那批东西在剪藏库里,抽屉再摆一遍是重复 */
export function pending(): HistoryItem[] {
  return items.filter((i) => !i.saved);
}

/** 清空。**只有用户按下抽屉里那个"清空"按钮时才会调** */
export function clear(): void {
  items = [];
  seq = 0;
}
