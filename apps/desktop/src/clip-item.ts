import { highlight } from "./highlight";
import type { MarkRange, SubText } from "./highlight";
import { t } from "./i18n";
import type { ClipSummary } from "./types";

/**
 * 列表里的一条剪藏。
 *
 * 单独成个模块的理由跟 `list.ts` / `undelete.ts` 那几个一样:`main.ts`
 * 一 import 就 boot(顶层就往 document 上挂东西),而这一条是**整个界面上
 * 被调用次数最多的函数**——两千篇的库进一次列表视图,它就是两千次。
 *
 * 更要紧的是它得能被量。前端渲染耗时以前从来没实测过,因为这段代码
 * 从 `main.ts` 里拿不出来,只能说"应该不慢"。抽出来之后
 * `clip-item.perf.test.ts` 能真的拿两千条数据把它跑一遍。
 *
 * 跟外面的耦合全走 `ClipItemContext`:谁被选中、哪篇正开着、点下去
 * 干什么。那些是**列表的状态**,不是这一条的属性,不该焊死在一条的
 * 构造过程里。
 */

/** 列表那边借给这一条用的几件事。 */
export interface ClipItemContext {
  /** 这一篇是不是详情页里正开着的那篇。 */
  isActive: (filename: string) => boolean;
  /** 在不在当前选区里。 */
  isSelected: (filename: string) => boolean;
  /** 点了一下(已经是"普通点击"了,按下 Ctrl/Shift 的分支不归这里管)。 */
  onOpen: (event: MouseEvent, filename: string) => void;
  onToggleRead: (clip: ClipSummary, next: boolean) => void;
  onToggleStar: (clip: ClipSummary, next: boolean) => void;
}

/** 剪藏时间。同一天只显示时分,列表扫起来更快。 */
export function formatWhen(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const now = new Date();
  if (d.toDateString() === now.toDateString()) {
    return d.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
  }
  return d.toLocaleDateString("zh-CN", { month: "2-digit", day: "2-digit" });
}

/** 列表里的一条。`readToggle` 关掉时不给「标已读」——回收站里没有已读这回事,
 *  给一堆删掉的剪藏挂个已读按钮,只会让人以为还能标。 */
export function clipItem(
  clip: ClipSummary,
  sub: SubText | null,
  opts: { readToggle?: boolean; titleMarks?: MarkRange[]; fuzzy?: boolean } = {},
  ctx: ClipItemContext,
): HTMLElement {
  const item = document.createElement("article");
  item.className = "clip";
  item.dataset.filename = clip.filename;
  if (ctx.isActive(clip.filename)) item.classList.add("active");
  if (clip.read || clip.archived) item.classList.add("done");
  // 读了一半要在列表里看得见,不然「读到哪儿了」这个功能只有自己知道。
  // 绝对定位,视觉上在右上角;**挂到 DOM 末尾**是为了读屏软件先念标题
  let halfRead: HTMLElement | null = null;
  if (clip.progress > 0 && !clip.read && !clip.archived) {
    halfRead = document.createElement("span");
    halfRead.className = "clip-half";
    halfRead.textContent = `${Math.round(clip.progress * 100)}%`;
  }

  const title = document.createElement("h3");
  title.className = "clip-title";
  // 近似命中的标。**塞进标题里、排在标题文字前面**:用户看的是
  // "这一条跟别的有什么不一样",不是标题本身。挂到卡片外层会另起一行,
  // 那行字把标题往下顶,扫列表时更费劲
  if (opts.fuzzy) {
    const tag = document.createElement("span");
    tag.className = "clip-fuzzy";
    tag.textContent = t("list.fuzzyTag");
    tag.title = t("list.fuzzyTag.title");
    title.append(tag);
  }
  // 标题来自网页,必须走转义。高亮也是**逐段拼**的,不给 innerHTML 开口子
  title.append(...highlight(clip.title, opts.titleMarks ?? []));

  const meta = document.createElement("p");
  meta.className = "clip-meta";
  const site = document.createElement("span");
  site.className = "clip-site";
  site.textContent = clip.site;
  const when = document.createElement("time");
  when.textContent = formatWhen(clip.clippedAt);
  meta.append(site, when);

  item.append(title, meta);

  // 收藏的星。**挂成真按钮而不是一个静态符号**——用户在列表里一眼就想
  // 取消收藏,让他得先点开文章再找开关,那叫让人多绕两步
  if (clip.starred) {
    const star = document.createElement("button");
    star.className = "star-toggle on";
    star.textContent = "★";
    star.title = t("clip.unstar.title");
    star.setAttribute("aria-pressed", "true");
    star.addEventListener("click", (event) => {
      event.stopPropagation(); // 不冒泡,不然会连带打开这一篇
      ctx.onToggleStar(clip, false);
    });
    item.append(star);
  }
  if (sub) {
    const excerpt = document.createElement("p");
    excerpt.className = "clip-excerpt";
    // **逐段拼,不走 innerHTML。** 标题和摘要都来自用户浏览过的网页,
    // 那里面可能有 `<script>`。走 innerHTML 高亮的那一刻就等于开了个口子
    excerpt.append(...highlight(sub.text, sub.marks));
    item.append(excerpt);
  }

  if (ctx.isSelected(clip.filename)) item.classList.add("selected");
  if (opts.readToggle === false) {
    if (halfRead) item.append(halfRead);
    item.addEventListener("click", (e) => ctx.onOpen(e, clip.filename));
    return item;
  }

  // 已读开关。**不自动标已读**——打开列表就把一堆剪藏刷成已读,
  // 那等于替用户做决定,而且不可逆(他可能只是误点了一下)。
  const toggle = document.createElement("button");
  toggle.className = "read-toggle";
  const done = clip.read || clip.archived;
  toggle.textContent = done ? t("clip.read") : t("clip.markRead");
  toggle.title = done ? t("clip.markUnread.title") : t("clip.markRead.title");
  toggle.setAttribute("aria-pressed", String(done));
  toggle.addEventListener("click", (event) => {
    // 不冒泡:不然会连带触发下面的 openDetail,读一篇却把列表刷没了
    event.stopPropagation();
    ctx.onToggleRead(clip, !clip.read);
  });
  item.append(toggle);
  if (halfRead) item.append(halfRead);

  item.addEventListener("click", (e) => ctx.onOpen(e, clip.filename));
  return item;
}
