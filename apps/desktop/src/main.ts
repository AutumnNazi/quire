import "./style.css";
import { listen } from "@tauri-apps/api/event";
import { api } from "./api";
import type { ClipboardCapture } from "./clipboard";
import {
  applyI18n,
  isWireError,
  localeName,
  otherLocale,
  setLocale,
  t,
  wireText,
} from "./i18n";
import { clipToMarkdown, MARKDOWN_PLACEHOLDER, renderMarkdown } from "./markdown";
import {
  groupByWeek,
  moveSelection,
  pruneSelection,
  reanchorAfterRemoval,
  selectAll,
  selectClips,
  selectRange,
  selectByTag,
  toggleSelected,
  upsertClip,
  withTag,
  withoutTag,
  type ListMode,
  type Selection,
} from "./list";
import type {
  BatchReport,
  ClipContent,
  ClipSummary,
  SearchHit,
  TrashItem,
  VaultInfo,
  TagCount,
} from "./types";

const mount = document.querySelector<HTMLDivElement>("#app");
if (!mount) throw new Error("页面缺少 #app 挂载点");
/** 挂载点。**提成 const 是为了让 TS 保留非空收窄**——收窄只写在
 *  顶层那一行,函数体里再引用 `root` 的话 TS 又当成可能是 null。 */
const root: HTMLDivElement = mount;

root.innerHTML = `
  <header class="toolbar">
    <button class="btn primary" id="btn-paste" data-i18n="toolbar.paste" data-i18n-title="toolbar.paste.title"></button>
    <span class="brand">Quire</span>
    <span class="vault-path" id="vault-path" data-i18n-title="toolbar.vaultPath.title"></span>
    <span class="spacer"></span>
    <input
      class="search"
      id="search"
      type="search"
      autocomplete="off"
      spellcheck="false"
      data-i18n-placeholder="toolbar.search.placeholder"
      data-i18n-title="toolbar.search.title"
    />
    <div class="filters" role="tablist" data-i18n-aria="toolbar.filters.aria">
      <button class="filter active" id="filter-all" role="tab" aria-selected="true" data-i18n="filter.all"></button>
      <button class="filter" id="filter-unread" role="tab" aria-selected="false" data-i18n="filter.unread" data-i18n-title="filter.unread.title"></button>
      <button class="filter" id="filter-week" role="tab" aria-selected="false" data-i18n="filter.week" data-i18n-title="filter.week.title"></button>
      <button class="filter" id="filter-archived" role="tab" aria-selected="false" data-i18n="filter.archived" data-i18n-title="filter.archived.title"></button>
      <button class="filter" id="filter-trash" role="tab" aria-selected="false" data-i18n="filter.trash" data-i18n-title="filter.trash.title"></button>
    </div>
    <label class="watch-toggle" data-i18n-title="watch.title">
      <input type="checkbox" id="chk-watch" />
      <span data-i18n="watch.label"></span>
    </label>
    <button class="btn" id="btn-import" data-i18n="toolbar.import" data-i18n-title="toolbar.import.title"></button>
    <button class="btn" id="btn-export" data-i18n="toolbar.export" data-i18n-title="toolbar.export.title"></button>
    <button class="btn" id="btn-open" data-i18n="toolbar.open"></button>
    <button class="btn" id="btn-pick" data-i18n="toolbar.pick"></button>
    <button class="btn ghost lang" id="btn-lang" data-i18n="lang.name" data-i18n-title="lang.switch"></button>
    <span class="shortcut-hint" data-i18n-title="toolbar.shortcut.title" data-i18n-html="toolbar.shortcut.text"></span>
  </header>
  <main class="split">
    <aside class="list-pane">
      <div class="warn" id="warn" hidden></div>
      <div class="tag-bar" id="tag-bar" hidden></div>
      <div class="list" id="list"></div>
      <div class="batch-bar" id="batch-bar" hidden>
        <span class="count" id="batch-count"></span>
        <button class="btn" id="batch-read" data-i18n="batch.read"></button>
        <button class="btn" id="batch-unread" data-i18n="batch.unread"></button>
        <button class="btn" id="batch-archive" data-i18n="batch.archive"></button>
        <button class="btn danger" id="batch-delete" data-i18n="batch.delete"></button>
        <button class="btn ghost" id="batch-clear" data-i18n="batch.clear"></button>
      </div>
    </aside>
    <section class="detail-pane" id="detail"></section>
  </main>
  <div class="toast" id="toast" hidden>
    <span class="toast-text" id="toast-text"></span>
    <span class="toast-actions" id="toast-actions" hidden></span>
    <button class="btn close" id="toast-close" data-i18n-aria="toast.close">×</button>
  </div>
`;

// 文案在这一步填进去。**不是把文案插进上面的模板**:模板里写死中文的话,
// 换语言就得重画整个骨架,而骨架里还散着一堆 id 引用,重画一次错一个
applyI18n(root);

const el = <T extends HTMLElement>(id: string): T => {
  const node = document.getElementById(id);
  if (!node) throw new Error(`缺少节点 #${id}`);
  return node as T;
};

const listEl = el<HTMLDivElement>("list");
const batchBarEl = el<HTMLDivElement>("batch-bar");
const batchCountEl = el<HTMLSpanElement>("batch-count");
const detailEl = el<HTMLElement>("detail");
const warnEl = el<HTMLDivElement>("warn");
const tagBarEl = el<HTMLDivElement>("tag-bar");
const vaultPathEl = el<HTMLSpanElement>("vault-path");
const toastEl = el<HTMLDivElement>("toast");
const toastTextEl = el<HTMLSpanElement>("toast-text");
const toastActionsEl = el<HTMLSpanElement>("toast-actions");
const watchEl = el<HTMLInputElement>("chk-watch");
const searchEl = el<HTMLInputElement>("search");
const filterAllEl = el<HTMLButtonElement>("filter-all");
const filterUnreadEl = el<HTMLButtonElement>("filter-unread");
const filterWeekEl = el<HTMLButtonElement>("filter-week");
const filterArchivedEl = el<HTMLButtonElement>("filter-archived");
const filterTrashEl = el<HTMLButtonElement>("filter-trash");
const langBtn = el<HTMLButtonElement>("btn-lang");

let clips: ClipSummary[] = [];
/** 回收站里的东西。**进回收站视图时才去拉**,平时不占着一次 IPC 往返。 */
let trash: TrashItem[] = [];
/** 多选。空集合 = 没在选,界面上不出现批量操作条。 */
let selected: Selection = new Set();
/** 滚动停止多久之后才把进度写盘。写盘会换掉整个文件(临时文件 + 改名),
 * 也会吵醒文件监控,滚动条每抖一下就写一次是灾难。1.5 秒是"用户停手了"
 * 和"用户还在慢慢读"之间比较稳的分界。 */
const PROGRESS_SAVE_DELAY = 1500;
let progressTimer: ReturnType<typeof setTimeout> | null = null;
/** Shift 连选的锚点。`null` 表示还没点过起点——这时按 Shift 只选当前那条。 */
let selectionAnchor: string | null = null;
/** 非空时列表显示的是检索结果,而不是全库。空数组和 null 要分清:
 *  null = 没在搜,空数组 = 搜了但一条没中,两者界面不一样。 */
let hits: SearchHit[] | null = null;
let activeFilename: string | null = null;
/** 当前详情页的归档按钮。归档之后要改它的文案,但重渲染整篇正文
 *  会把滚动位置弹回顶部、还要再解析一遍 Markdown,所以就地改这一个节点。 */
let archiveBtn: HTMLButtonElement | null = null;
/** 当前筛选。搜索和筛选是**两回事**:搜出来的结果不再过筛,否则用户
 *  搜到一篇却看不见,只会当成搜索坏了。 */
let filter: ListMode | "trash" = "all";
/** 监控弹出来的内容。等用户点"保存"时才真正落盘——自动存等于替他做决定。 */
let pendingCapture: ClipboardCapture | null = null;
let toastTimer: ReturnType<typeof setTimeout> | null = null;
let searchTimer: ReturnType<typeof setTimeout> | null = null;

function showError(message: string): void {
  warnEl.textContent = message;
  warnEl.hidden = false;
}

/** 把后端送回来的错误翻成当前语言的句子,再挂到红条上。
 *
 *  所有 `catch` 都走这里,不再各自 `String(err)`——那写法对新的错误类型
 *  会打出 `[object Object]`,对字符串错误又没法加上下文,两头都不对。 */
function showBackendError(
  key: string,
  err: unknown,
  extra: Record<string, string | number> = {},
): void {
  // 不是后端的错误(比如 `invoke` 本身失败)时 `wireText` 会退回一句字符串,
  // 所以这一处不用再分叉
  showError(t(key, { ...extra, detail: wireText(err) }));
}

function clearError(): void {
  warnEl.hidden = true;
  warnEl.textContent = "";
}

interface ToastAction {
  label: string;
  primary?: boolean;
  onClick: () => void;
}

/** 弹一条提示。带按钮的等用户处理,不自动消失——**按钮消失的那一下,
 *  用户就还没来得及反应**,剪藏提示、删除撤销都栽在这上面过。 */
function showToast(text: string, actions: ToastAction[] = []): void {
  if (toastTimer) clearTimeout(toastTimer);
  toastTextEl.textContent = text;
  toastActionsEl.replaceChildren(
    ...actions.map((action) => {
      const button = document.createElement("button");
      button.className = action.primary ? "btn primary" : "btn";
      button.textContent = action.label;
      button.addEventListener("click", action.onClick);
      return button;
    }),
  );
  toastActionsEl.hidden = actions.length === 0;
  toastEl.hidden = false;
  if (actions.length === 0) {
    toastTimer = setTimeout(() => {
      toastEl.hidden = true;
    }, 2600);
  }
}

function hideToast(): void {
  if (toastTimer) clearTimeout(toastTimer);
  toastEl.hidden = true;
  pendingCapture = null;
}

/** 剪藏时间。同一天只显示时分,列表扫起来更快。 */
function formatWhen(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const now = new Date();
  if (d.toDateString() === now.toDateString()) {
    return d.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
  }
  return d.toLocaleDateString("zh-CN", { month: "2-digit", day: "2-digit" });
}

/** 当前屏幕上真的列出来的那些剪藏,按显示顺序。
 *
 *  快捷键只认这个列表——认 `clips` 的话,「未读」视图下按下一条会跳到
 *  一篇根本没显示的文章上,用户看着屏幕会以为软件坏了。 */
function visibleFilenames(): string[] {
  return [...listEl.querySelectorAll<HTMLElement>(".clip")].map(
    (el) => el.dataset.filename ?? "",
  ).filter(Boolean);
}

function renderList(): void {
  renderListInner();
  // 列表重画完才知道哪些还看得见。**先清后画**是不行的:那时列表已经被
  // replaceChildren 清空了,算出来的"可见项"永远是空集,一选就被清光
  selected = pruneSelection(selected, visibleFilenames());
  for (const el of listEl.querySelectorAll<HTMLElement>(".clip.selected")) {
    const name = el.dataset.filename;
    if (!name || !selected.has(name)) el.classList.remove("selected");
  }
  renderBatchBar();
}

function renderListInner(): void {
  listEl.replaceChildren();

  // 回收站是**另一份数据**,不是 clips 的一个筛选。走前面的每条路径之前先岔开:
  // 搜索、未读、每周,说的全是"库里还有什么",跟"删掉的还剩什么"没关系
  if (filter === "trash") {
    renderTrashList();
    return;
  }

  if (hits) {
    if (hits.length === 0) {
      const none = document.createElement("div");
      none.className = "empty";
      none.innerHTML = `<p class="empty-title">${t("list.searchNone.title")}</p><p>${t("list.searchNone.body")}</p>`;
      listEl.append(none);
      return;
    }
    renderHitList(hits);
    return;
  }

  if (clips.length === 0) {
    const empty = document.createElement("div");
    empty.className = "empty";
    empty.innerHTML = `
      <p class="empty-title">${t("list.empty.title")}</p>
      <p>${t("list.empty.body")}</p>
    `;
    listEl.append(empty);
    return;
  }

  // 「每周」单独走分组渲染。其余两种筛选都是同一份列表,只是筛得不一样。
  if (filter === "week") {
    renderWeekList();
    return;
  }

  // 标签筛选跟「全部 / 未读 / 每周 / 归档」是**正交**的:能叠着用,
  // 「未读」里只看「待读」正是最常见的用法。搜出来的结果不过这道筛——
  // 搜到了却看不见,用户只会当成搜索坏了
  const shown = selectByTag(selectClips(clips, filter), tagFilter);
  if (shown.length === 0) {
    const none = document.createElement("div");
    none.className = "empty";
    // 三种空态三种说法。「没有内容」这种话等于让用户以为剪藏丢了。
    if (tagFilter !== null) {
      // 单独一种说法。这时的空不是"库里没有",是"这一类下暂时没有",
      // 混进「没有未读了」那种话里,用户会以为剪藏全没了
      // **走 textContent 不走 innerHTML**:标签是用户自己起的,也可能来自
      // 导入的 .md,一个 `<img onerror=…>` 塞进去就执行了
      const title = document.createElement("p");
      title.className = "empty-title";
      title.textContent = t("list.tagEmpty.title", { tag: tagFilter });
      const body = document.createElement("p");
      body.textContent = t("list.tagEmpty.body");
      none.replaceChildren(title, body);
      listEl.append(none);
      return;
    } else {
      none.innerHTML =
        filter === "unread"
          ? `<p class="empty-title">${t("list.unreadEmpty.title")}</p><p>${t("list.unreadEmpty.body")}</p>`
          : filter === "archived"
            ? `<p class="empty-title">${t("list.archivedEmpty.title")}</p><p>${t("list.archivedEmpty.body")}</p>`
            : `<p class="empty-title">${t("list.none.title")}</p>`;
    }
    listEl.append(none);
    return;
  }

  for (const clip of shown) {
    listEl.append(clipItem(clip, clip.excerpt));
  }
}

/** 每周回顾:按 ISO 自然周分组,组头写清「共几篇、还剩几篇没读」。
 *  未读数是重点——回顾的意义就是提醒你还欠自己多少。 */
function renderWeekList(): void {
  const groups = groupByWeek(clips);
  if (groups.length === 0) {
    const none = document.createElement("div");
    none.className = "empty";
    none.innerHTML = `<p class="empty-title">${t("list.weekEmpty.title")}</p>`;
    listEl.append(none);
    return;
  }
  for (const group of groups) {
    const header = document.createElement("div");
    header.className = "week-header";
    const unread = group.clips.filter((c) => !c.read && !c.archived).length;
    // 整组都读完了就别再报未读,挂个 0 只会让人多看一眼
    const suffix = unread > 0 ? t("list.weekUnread", { n: unread }) : t("list.weekAllRead");
    header.textContent = t("list.weekHeader", {
      year: group.isoYear,
      week: group.isoWeek,
      count: group.clips.length,
      suffix,
    });
    listEl.append(header);
    for (const clip of selectClips(group.clips, "all")) {
      listEl.append(clipItem(clip, clip.excerpt));
    }
  }
}

/** 检索态下列表长这样:标题、站点时间,外加一段命中上下文。
 *  正文摘要在这儿没用——用户搜的就是这几个字,得让他看见它们出现在哪儿。 */
function renderHitList(results: SearchHit[]): void {
  for (const hit of results) {
    listEl.append(clipItem(hit.summary, hit.snippet, { readToggle: filter !== "trash" }));
  }
}

/** 回收站列表。**读不出元数据的也要列出来**——那正是最该被看见、
 *  也最该能被清掉的一批,藏起来等于永远清不掉。
 *
 *  不复用 `clipItem`:那边带「标已读」开关,回收站里没有已读这回事,
 *  给一堆删掉的剪藏挂个"已读"按钮只会让人以为还能标。 */
function renderTrashList(): void {
  if (trash.length === 0) {
    const none = document.createElement("div");
    none.className = "empty";
    none.innerHTML = `<p class="empty-title">${t("trash.empty.title")}</p>`;
    listEl.append(none);
    return;
  }
  const total = trash.reduce((sum, item) => sum + item.sizeBytes, 0);
  const header = document.createElement("div");
  header.className = "week-header trash-header";
  const label = document.createElement("span");
  label.textContent = t("trash.header", { count: trash.length, size: formatBytes(total) });
  // 清空摆在**它作用的东西旁边**,不占工具栏。工具栏是全局的,
  // 在那儿长期摆一个红按钮,用户会怕点错,也把工具栏挤不下了
  const empty = document.createElement("button");
  empty.className = "btn danger";
  empty.textContent = t("trash.emptyTrash");
  empty.title = t("trash.emptyTrash.title");
  empty.addEventListener("click", () => askEmptyTrash());
  header.append(label, empty);
  listEl.append(header);

  for (const item of trash) listEl.append(trashItem(item));
}

function trashItem(item: TrashItem): HTMLElement {
  const el = document.createElement("article");
  el.className = "clip";
  el.dataset.filename = item.filename;
  if (item.filename === activeFilename) el.classList.add("active");

  const title = document.createElement("h3");
  title.className = "clip-title";
  title.textContent = item.summary?.title ?? t("trash.untitled");
  el.append(title);

  const meta = document.createElement("p");
  meta.className = "clip-meta";
  const site = document.createElement("span");
  site.className = "clip-site";
  // 元数据读不出来时把文件名摆出来。那是用户自己在文件管理器里能认出的
  // 唯一线索,也是判断"这条要不要删"的依据
  site.textContent = item.summary?.site ?? item.filename;
  const when = document.createElement("time");
  when.textContent = item.summary ? formatWhen(item.summary.clippedAt) : formatBytes(item.sizeBytes);
  meta.append(site, when);
  el.append(meta);

  if (selected.has(item.filename)) el.classList.add("selected");
  el.addEventListener("click", (e) => onItemClick(e, item.filename));
  return el;
}

/** 体积显示。回收站不是免费的,它占着磁盘,用户有权知道占了多少。 */
function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/** 列表里的一条。`readToggle` 关掉时不给「标已读」——回收站里没有已读这回事,
 *  给一堆删掉的剪藏挂个已读按钮,只会让人以为还能标。 */
function clipItem(
  clip: ClipSummary,
  sub: string | null,
  opts: { readToggle?: boolean } = {},
): HTMLElement {
  const item = document.createElement("article");
  item.className = "clip";
  item.dataset.filename = clip.filename;
  if (clip.filename === activeFilename) item.classList.add("active");
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
  title.textContent = clip.title; // textContent 而非 innerHTML:标题来自网页,必须走转义

  const meta = document.createElement("p");
  meta.className = "clip-meta";
  const site = document.createElement("span");
  site.className = "clip-site";
  site.textContent = clip.site;
  const when = document.createElement("time");
  when.textContent = formatWhen(clip.clippedAt);
  meta.append(site, when);

  item.append(title, meta);
  if (sub) {
    const excerpt = document.createElement("p");
    excerpt.className = "clip-excerpt";
    excerpt.textContent = sub;
    item.append(excerpt);
  }

  if (clip.filename && selected.has(clip.filename)) item.classList.add("selected");
  if (opts.readToggle === false) {
    if (halfRead) item.append(halfRead);
    item.addEventListener("click", (e) => onItemClick(e, clip.filename));
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
    void toggleRead(clip, !clip.read);
  });
  item.append(toggle);
  if (halfRead) item.append(halfRead);

  item.addEventListener("click", (e) => onItemClick(e, clip.filename));
  return item;
}

/** 列表项点击。`Ctrl`/`Cmd` 是加选减选,`Shift` 是连选,都不打开正文——
 *  用户按住这两个键是在"挑一堆",不是在"读一篇"。 */
function onItemClick(event: MouseEvent, filename: string): void {
  if (event.ctrlKey || event.metaKey) {
    selected = toggleSelected(selected, filename);
    selectionAnchor = filename;
  } else if (event.shiftKey) {
    selected = selectRange(visibleFilenames(), selectionAnchor, filename, selected);
    renderList();
    return;
  } else {
    selected = new Set();
    selectionAnchor = null;
  }
  renderList();
  // 只有"普通单击"才打开正文。按住 Ctrl 把最后一项取消掉、顺带把文章弹出来,
  // 那是用户没要求的
  void (filter === "trash" ? openTrashDetail(filename) : openDetail(filename));
}

/** 渲染批量操作条。**只在库视图里出现**——回收站和搜索结果各有各的一套动作,
 *  把「标已读」摆在回收站上等于摆一个按了没反应的按钮。 */
function renderBatchBar(): void {
  const on = selected.size > 0 && filter !== "trash";
  batchBarEl.hidden = !on;
  if (!on) return;
  batchCountEl.textContent = t("batch.count", { n: selected.size });
}

/** 改已读标志。失败要说出来——静默失败的代价是用户以为标上了,
 *  文件其实没动,下次打开又变回未读,他只会觉得 Quire 有毛病。 */
async function toggleRead(clip: ClipSummary, next: boolean): Promise<void> {
  try {
    const updated = await api.setClipFlags(clip.filename, next);
    // 就地改列表里那一份,别整份重扫磁盘——列表小的时候重扫会明显卡一下
    const i = clips.findIndex((c) => c.filename === clip.filename);
    if (i >= 0) clips[i] = updated;
    if (hits) {
      for (const hit of hits) {
        if (hit.summary.filename === clip.filename) hit.summary = updated;
      }
    }
    renderList();
  } catch (err) {
    showBackendError("error.markReadFailed", err);
  }
}

/** 归档/取消归档。归档是「挪到一边」不是「删掉」,所以要有个地方能翻回来——
 *  列表里的「归档」筛选就是那个地方。放在正文页而不是列表项上,是因为
 *  「看完了不打算再留」是个读完之后的决定,不是扫一眼列表时顺手做的。 */
async function toggleArchive(filename: string): Promise<void> {
  const before = clips.find((c) => c.filename === filename);
  if (!before) return;
  try {
    const updated = await api.setClipFlags(filename, undefined, !before.archived);
    clips = upsertClip(clips, updated);
    if (hits) {
      for (const hit of hits) {
        if (hit.summary.filename === filename) hit.summary = updated;
      }
    }
    // 详情页那一份可能还是旧值,刷新一下,不然再点一次按钮会拿旧的
    // archived 取反,变成"点了没反应"
    if (activeFilename === filename) await refreshOpenDetail();
    renderList();
  } catch (err) {
    showBackendError(
      before.archived ? "error.unarchiveFailed" : "error.archiveFailed",
      err,
    );
  }
}

/** 重新拉一次正在看的那篇。
 *
 *  改完标志之后,详情页手上那份 `ClipContent` 还是旧值——不刷的话再点一次
 *  归档按钮会拿旧的 `archived` 取反,用户看着就是"点了没反应"。这里重读
 *  一次,顺带把滚动位置留着,不然改个标志正文就弹回顶部了。 */
async function refreshOpenDetail(): Promise<void> {
  if (!activeFilename) return;
  const filename = activeFilename;
  const scroll = detailEl.scrollTop;
  try {
    const clip = await api.readClip(filename);
    if (activeFilename !== filename) return;
    renderDetail(clip);
    detailEl.scrollTop = scroll;
  } catch (err) {
    showBackendError("error.readFailed", err);
  }
}

/** 移到回收站。**不真删**——剪藏工具里唯一能把用户东西弄没的操作,
 *  没有必要一按就没。真想清空,用户自己去 `clips/.trash/` 里翻,那时候他
 *  是想清楚了才翻的。 */
async function trashClip(filename: string): Promise<void> {
  const report = await trashMany([filename]);
  if (report.failed.length > 0) {
    showError(t("error.trashFailed", { detail: wireText(report.failed[0].reason) }));
    return;
  }
  showToast(t("toast.movedToTrash"), [
    { label: t("toast.undo"), primary: true, onClick: () => void undoTrash(filename) },
    { label: t("toast.close"), onClick: hideToast },
  ]);
}

/** 一次删多篇。**成功和失败必须分开报**:一次删 20 篇里有 2 篇没删成,
 * 只报一句"已删除"的话,用户会以为回收站里那两篇还在,回头找时才发现。 */
async function trashMany(filenames: string[]): Promise<BatchReport> {
  if (filenames.length === 0) return { succeeded: [], failed: [] };
  const report = await api.trashClips(filenames);
  const gone = new Set(report.succeeded);
  clips = clips.filter((c) => !gone.has(c.filename));
  if (hits) hits = hits.filter((h) => !gone.has(h.summary.filename));
  selected = new Set([...selected].filter((f) => !gone.has(f)));
  if (activeFilename && gone.has(activeFilename)) {
    activeFilename = null;
    renderEmptyDetail();
  }
  renderList();
  return report;
}

async function undoTrash(filename: string): Promise<void> {
  hideToast();
  try {
    const back = await api.restoreClip(filename);
    // 走 upsert 而不是 `[back, ...clips]`:放回来的是一篇旧剪藏,
    // 顶到列表最前面的话,撤销一次顺序就乱一次
    clips = upsertClip(clips, back);
    renderList();
    showToast(t("toast.restored"));
  } catch (err) {
    // 放不回来是真出了岔子,不能当成没事发生——用户会以为东西回来了
    showBackendError("error.undoFailed", err);
  }
}

function syncArchiveButton(clip: ClipContent): void {
  if (!archiveBtn) return;
  archiveBtn.textContent = clip.archived ? t("detail.unarchive") : t("detail.archive");
  archiveBtn.classList.toggle("active", clip.archived);
  archiveBtn.setAttribute("aria-pressed", String(clip.archived));
  archiveBtn.title = clip.archived
    ? t("detail.unarchive.title")
    : t("detail.archive.title");
}

function renderDetail(clip: ClipContent): void {
  detachProgress();
  detailEl.replaceChildren();
  archiveBtn = null;

  const header = document.createElement("header");
  header.className = "detail-head";

  const title = document.createElement("h1");
  title.textContent = clip.title;

  const meta = document.createElement("p");
  meta.className = "detail-meta";
  meta.append(document.createTextNode(clip.site));
  const when = document.createElement("time");
  when.textContent = clip.clippedAt.replace("T", " ").replace(/\+.*$/, "");
  meta.append(when);

  // 剪藏带回来的原文地址才是回溯的入口,没拿到就不渲染链接,
  // 总比给个指向空白页的假入口强
  if (clip.url && /^https?:\/\//i.test(clip.url)) {
    const link = document.createElement("a");
    link.href = clip.url;
    link.target = "_blank";
    link.rel = "noopener noreferrer";
    link.textContent = t("detail.openOriginal");
    meta.prepend(link);
  }

  // 进度条。**放在正文最上面而不是文章末尾**:读到哪儿了这件事,
  // 得在滚动时一眼看见,而不是滚到底才知道
  const progress = document.createElement("div");
  progress.className = "read-progress";
  progress.title = t("detail.progress.title");
  const bar = document.createElement("i");
  progress.append(bar);

  const body = document.createElement("article");
  body.className = "prose";
  // 唯一使用 innerHTML 的地方,内容已过 DOMPurify
  body.innerHTML = renderMarkdown(clip.body);
  for (const img of body.querySelectorAll("img")) {
    // 不发 Referer,免得用户读了什么被图片服务器记录去
    img.referrerPolicy = "no-referrer";
    img.loading = "lazy";
    img.addEventListener(
      "error",
      () => {
        img.src = MARKDOWN_PLACEHOLDER;
      },
      { once: true },
    );
  }

  const actions = document.createElement("div");
  actions.className = "detail-actions";
  const archive = document.createElement("button");
  archive.className = "btn ghost";
  archiveBtn = archive;
  archive.addEventListener("click", () => void toggleArchive(clip.filename));
  const remove = document.createElement("button");
  remove.className = "btn danger";
  remove.textContent = t("detail.delete");
  remove.title = t("detail.delete.title");
  remove.addEventListener("click", () => void trashClip(clip.filename));
  actions.append(archive, remove);
  syncArchiveButton(clip);

  header.append(title, meta, actions);
  detailEl.append(header, progress, tagEditor(clip), body);
  detailEl.scrollTop = 0;
  trackReadingProgress(clip.filename, clip.progress, bar);
}

/* ── 标签 ── */

/** 当前按哪个标签筛着。`null` = 没筛。跟 `filter` 是**两回事**:
 *  筛选切的是"库里还有什么",标签筛的是"带这个标签的还有哪些",
 *  两个是正交的,能叠在一起用(「未读」里只看「待读」)。 */
let tagFilter: string | null = null;
/** 标签栏的数据。改了标签、增删了剪藏之后都得重拉,不然标签栏上
 *  那个数字会一直停在旧值,用户点进去发现对不上。 */
let tags: TagCount[] = [];
/** 标签栏最多摆几个。**多出来的折叠成「还有 N 个」**:标签栏横向占位,
 *  摆满一屏的话列表本身就被挤到看不见了,而列表才是主菜。 */
const TAG_BAR_LIMIT = 12;

/** 详情页里的标签编辑器。已有的标签是可点的芯片(点了就摘掉),
 *  底下是一个输入框,敲完回车就加。
 *
 *  **加完标签不重画整篇正文**——那会把滚动位置弹回顶部、还要再解析一遍
 *  Markdown。就地重画这一小块。 */
function tagEditor(clip: ClipContent): HTMLElement {
  const box = document.createElement("div");
  box.className = "tag-editor";

  const chips = document.createElement("div");
  chips.className = "tag-chips";
  chips.append(...clip.tags.map((tag) => tagChip(tag, () => void applyTags(clip, withoutTag(clip.tags, tag)))));

  const form = document.createElement("form");
  form.className = "tag-add";
  const input = document.createElement("input");
  input.type = "text";
  input.placeholder = t("tag.edit.placeholder");
  input.setAttribute("aria-label", t("tag.edit.add"));
  const add = document.createElement("button");
  add.className = "btn";
  add.type = "submit";
  add.textContent = t("tag.edit.add");
  form.append(input, add);
  form.addEventListener("submit", (e) => {
    // 不拦住的话回车会把整个详情页刷新掉
    e.preventDefault();
    const value = input.value.trim();
    if (!value) return;
    input.value = "";
    void applyTags(clip, withTag(clip.tags, value));
  });

  const hint = document.createElement("p");
  hint.className = "tag-hint";
  hint.textContent = t("tag.edit.hint");

  box.append(chips, form, hint);
  return box;
}

function tagChip(tag: string, onRemove: () => void): HTMLButtonElement {
  const chip = document.createElement("button");
  chip.className = "tag-chip removable";
  chip.type = "button";
  chip.textContent = tag;
  chip.title = t("tag.edit.remove", { tag });
  chip.setAttribute("aria-label", t("tag.edit.remove", { tag }));
  chip.addEventListener("click", onRemove);
  return chip;
}

/** 改标签然后把受影响的界面都更新一遍。**失败要说出来**:
 *  标签没存上而界面显示存上了,用户回头按标签找,那批文章不在,
 *  他只会觉得标签功能坏了。 */
async function applyTags(clip: ClipContent, next: string[]): Promise<void> {
  try {
    const updated = await api.setClipTags(clip.filename, next);
    clips = upsertClip(clips, updated);
    if (hits) {
      for (const hit of hits) {
        if (hit.summary.filename === updated.filename) hit.summary = updated;
      }
    }
    if (activeFilename === updated.filename) {
      // 就地重画编辑器这一块,别碰正文——重画正文会把滚动位置弹回顶部
      const box = detailEl.querySelector(".tag-editor");
      if (box) box.replaceWith(tagEditor({ ...clip, tags: updated.tags }));
    }
    tags = await api.listTags();
    renderTagBar();
    renderList();
  } catch (err) {
    showBackendError("error.tagFailed", err);
  }
}

/** 列表上方的标签栏。**归档的不算**(后端 `tag_index` 已经滤掉了):
 *  标签栏是给「还打算看的那些」用的,一堆归档标签混在里面,用户点着
 *  点着就以为标签乱了。 */
function renderTagBar(): void {
  if (!tagBarEl) return;
  // 回收站里没有标签这回事,摆一条空栏只会让人以为能按标签找删掉的东西
  tagBarEl.hidden = filter === "trash";
  if (tagBarEl.hidden) return;
  // 光有一排圆角小片,没人知道那是干什么的。鼠标停上去和读屏都要说清楚
  tagBarEl.title = t("tag.bar.title");
  tagBarEl.setAttribute("aria-label", t("tag.bar.title"));

  if (tags.length === 0) {
    const none = document.createElement("p");
    none.className = "tag-bar-empty";
    none.textContent = t("tag.bar.empty");
    tagBarEl.replaceChildren(none);
    return;
  }

  const shown = tags.slice(0, TAG_BAR_LIMIT);
  const rest = tags.length - shown.length;
  tagBarEl.replaceChildren(
    ...shown.map((item) => {
      const on = tagFilter === item.tag;
      const chip = document.createElement("button");
      chip.className = on ? "tag-chip filter active" : "tag-chip filter";
      chip.type = "button";
      chip.setAttribute("aria-pressed", String(on));
      chip.textContent = item.tag;
      // 数字比"归档的不算"更说明问题:点了才发现是 0,不如先摆着
      chip.title = on
        ? t("tag.filter.active", { tag: item.tag })
        : t("tag.count", { n: item.count });
      chip.addEventListener("click", () => {
        // 再点一下同一个 = 取消筛选。这是最基本的可逆操作,
        // 找不着出口的话用户会以为标签筛进去就出不来了
        tagFilter = on ? null : item.tag;
        renderTagBar();
        renderList();
      });
      return chip;
    }),
    ...(rest > 0
      ? [
          (() => {
            const more = document.createElement("span");
            more.className = "tag-bar-more";
            more.textContent = t("tag.bar.more", { n: rest });
            more.title = tags
              .slice(TAG_BAR_LIMIT)
              .map((i) => i.tag)
              .join("、");
            return more;
          })(),
        ]
      : []),
  );
}

/* ── 阅读进度 ── */

/** 当前详情页挂着的滚动监听。换一篇之前要先摘掉,不然读第二篇时
 *  第一篇的滚动也会往它自己的文件里写进度。 */
let progressScroll: { filename: string; onScroll: () => void } | null = null;

/** 读到哪儿了,0–1。
 *
 *  **一屏装得下就是 100%。** 短笔记没有"读一半"这回事,给它记 0.3 只会
 *  让列表里出现一条永远停在三分之一的长条。 */
function readingProgress(): number {
  const scrollable = detailEl.scrollHeight - detailEl.clientHeight;
  if (scrollable <= 8) return 1;
  return Math.min(1, detailEl.scrollTop / scrollable);
}

/** 挂上滚动监听,停手 1.5 秒后把进度写盘。
 *
 *  打开时先按文件里记的进度跳回去——"读到哪儿了"这个功能有一半的价值
 *  在于**重新打开时接着读**。`requestAnimationFrame` 是为了等布局稳定,
 *  刚 render 完就量 scrollHeight,拿到的可能还是上一篇的旧值。 */
function trackReadingProgress(filename: string, saved: number, bar: HTMLElement): void {
  detachProgress();
  bar.style.width = `${Math.round(saved * 100)}%`;
  if (saved > 0) {
    requestAnimationFrame(() => {
      const scrollable = detailEl.scrollHeight - detailEl.clientHeight;
      if (scrollable > 8) detailEl.scrollTop = scrollable * saved;
    });
  }

  const onScroll = (): void => {
    bar.style.width = `${Math.round(readingProgress() * 100)}%`;
    if (progressTimer) clearTimeout(progressTimer);
    progressTimer = setTimeout(() => {
      progressTimer = null;
      void saveProgress(filename, readingProgress());
    }, PROGRESS_SAVE_DELAY);
  };
  detailEl.addEventListener("scroll", onScroll, { passive: true });
  progressScroll = { filename, onScroll };
}

/** 摘掉进度监听和待写的定时器。**每个重画详情的地方都得调**:
 * 监听器挂在 `detailEl` 上而不是正文节点上,换一篇不会自动摘,
 * 读第二篇时第一篇的滚动也会往它自己的文件里写进度。 */
function detachProgress(): void {
  if (progressTimer) {
    clearTimeout(progressTimer);
    progressTimer = null;
  }
  if (progressScroll) {
    detailEl.removeEventListener("scroll", progressScroll.onScroll);
    progressScroll = null;
  }
}


async function saveProgress(filename: string, progress: number): Promise<void> {
  // 已经读到 100% 的,以后再打开也不该被"读了一半"的进度条盖住。
  // 后端只认"值变了才写",但界面这一侧也得先想清楚
  if (progress >= 0.999) progress = 1;
  try {
    await api.setClipProgress(filename, progress);
  } catch {
    // 记不住进度是小事,不该在读文章的时候弹一条红字打断
  }
}

function renderEmptyDetail(): void {
  detachProgress();
  detailEl.replaceChildren();
  archiveBtn = null; // 上一篇的按钮节点已经脱离文档,留着只会改空气
  const hint = document.createElement("div");
  hint.className = "empty";
  hint.innerHTML = `<p class="empty-title">${t("detail.pickOne")}</p>`;
  detailEl.append(hint);
}

async function openDetail(filename: string): Promise<void> {
  activeFilename = filename;
  renderList();
  try {
    const clip = await api.readClip(filename);
    if (activeFilename !== filename) return; // 用户点得比读得快,丢弃过期结果
    renderDetail(clip);
  } catch (err) {
    showBackendError("error.readFailed", err);
  }
}

/** 打开回收站里的一篇。**能看内容才谈得上"确认要不要永久删"**——
 *  「彻底删除」是整个软件里唯一没有撤销的操作,看不见就按下去那叫闭眼签字。 */
async function openTrashDetail(filename: string): Promise<void> {
  activeFilename = filename;
  renderList();
  archiveBtn = null;
  detachProgress();
  detailEl.replaceChildren();
  try {
    const clip = await api.readTrashClip(filename);
    if (activeFilename !== filename) return;
    renderTrashDetail(clip);
  } catch {
    // 元数据坏掉的文件照样要点得开、能删得掉。这里不弹红字,
    // 只说清实情,再把两个按钮摆出来
    if (activeFilename !== filename) return;
    const fallback = document.createElement("h1");
    fallback.textContent = t("trash.unreadable.title");
    const note = document.createElement("p");
    note.className = "detail-meta";
    note.textContent = t("trash.unreadable.body");
    detailEl.replaceChildren(fallback, note, trashActions(filename, null));
  }
}

function renderTrashDetail(clip: ClipContent): void {
  // 回收站里不记进度:**彻底删除之前**显示"你读到 80%"会让人以为还能接着读
  detachProgress();
  const header = document.createElement("header");
  header.className = "detail-header";
  const title = document.createElement("h1");
  title.textContent = clip.title;
  const meta = document.createElement("p");
  meta.className = "detail-meta";
  meta.append(document.createTextNode(clip.site));
  const when = document.createElement("time");
  when.textContent = clip.clippedAt.replace("T", " ").replace(/\+.*$/, "");
  meta.append(when);
  header.append(title, meta, trashActions(clip.filename, trash.find((t) => t.filename === clip.filename)?.sizeBytes ?? null));

  const body = document.createElement("article");
  body.className = "prose";
  // 唯一使用 innerHTML 的地方,内容已过 DOMPurify
  body.innerHTML = renderMarkdown(clip.body);
  for (const img of body.querySelectorAll("img")) {
    img.referrerPolicy = "no-referrer";
    img.loading = "lazy";
    img.addEventListener("error", () => { img.src = MARKDOWN_PLACEHOLDER; }, { once: true });
  }
  detailEl.replaceChildren(header, body);
  detailEl.scrollTop = 0;
}

/** 回收站里的两个动作。`sizeBytes` 是磁盘上真实占的量,拿它说"删掉能省多少";
 *  读不出内容的文件拿不到大小,就不显示这句。 */
function trashActions(filename: string, sizeBytes: number | null): HTMLElement {
  const actions = document.createElement("div");
  actions.className = "detail-actions";

  const back = document.createElement("button");
  back.className = "btn primary";
  back.textContent = t("trash.restore");
  back.title = t("trash.restore.title");
  back.addEventListener("click", () => void restoreFromTrash(filename));

  const purge = document.createElement("button");
  purge.className = "btn danger";
  purge.textContent = t("trash.purge");
  // 措辞要说清楚后果:没有撤销、没有回收站第二层
  purge.title = sizeBytes === null
    ? t("trash.purge.title")
    : t("trash.purge.titleWithSize", { size: formatBytes(sizeBytes) });
  purge.addEventListener("click", () => void purgeFromTrash(filename));

  actions.append(back, purge);
  return actions;
}

async function restoreFromTrash(filename: string): Promise<void> {
  try {
    const back = await api.restoreClip(filename);
    trash = trash.filter((t) => t.filename !== filename);
    // 放回来的是一篇旧剪藏,走 upsert 才不会每次撤销都把它顶到列表最前面
    clips = upsertClip(clips, back);
    if (activeFilename === filename) {
      // 放回来一篇之后接着看下一篇,而不是把详情页清空
      const at = visibleFilenames().indexOf(filename);
      activeFilename = null;
      renderTrashList();
      const next = reanchorAfterRemoval(visibleFilenames(), at);
      if (next) void openTrashDetail(next);
      else renderEmptyDetail();
    } else {
      renderTrashList();
    }
    showToast(t("toast.restored"));
  } catch (err) {
    // 放不回来是真出了岔子,不能当成没事发生——用户会以为东西回来了
    showBackendError("error.restoreFailed", err);
  }
}

async function purgeFromTrash(filename: string): Promise<void> {
  const at = visibleFilenames().indexOf(filename);
  try {
    await api.purgeClip(filename);
    trash = trash.filter((t) => t.filename !== filename);
    activeFilename = null;
    renderTrashList();
    // 删掉一篇之后选中它原来那个位置的下一篇,而不是跳回第一篇
    const next = reanchorAfterRemoval(visibleFilenames(), at);
    if (next) void openTrashDetail(next);
    else renderEmptyDetail();
    showToast(t("toast.purged"));
  } catch (err) {
    showBackendError("error.purgeFailed", err);
  }
}

async function refreshTrash(): Promise<void> {
  trash = (await api.listTrash()).items;
}

/** 重拉标签栏。**拉不到就清空**:留着上一次的数据等于告诉用户
 *  「这些标签现在还是这样」,而实际上已经变了——宁可空着让他重新点。 */
async function refreshTags(): Promise<void> {
  if (filter === "trash") return;
  try {
    tags = await api.listTags();
  } catch {
    tags = [];
  }
}

/** 存一篇剪藏。返回 `null` 表示存成了,返回文件名表示"已经剪过了"。
 *
 *  判重交给后端做——地址来自剪贴板,前端那份和库里那份没法保证同源。
 *  `force` 来自用户在重复提示里点了「仍然存一份」。 */
async function saveCapture(
  capture: ClipboardCapture,
  force = false,
): Promise<string | null> {
  const { markdown, title, excerpt, meta } = clipToMarkdown(capture);
  if (!markdown.trim()) {
    showError(t("error.clipboardNoContent"));
    return null;
  }
  const outcome = await api.saveClip(
    {
      schemaVersion: 1,
      url: capture.url ?? "",
      title,
      // 有扩展就用它给的站点显示名,没有就留空让后端从 URL 推主机名
      siteName: meta["quire-site"] ?? "",
      author: meta["quire-author"] ?? null,
      excerpt: excerpt || null,
      markdown,
      publishedAt: meta["quire-published"] ?? null,
      image: meta["quire-image"] ?? null,
      // 剪贴板里没有 favicon,存进去只会是个永远加载不出来的死链
      favicon: null,
    },
    force,
  );
  clearError();
  return outcome.status === "duplicate" ? outcome.filename : null;
}

/** 同一篇已经剪过了。用户复制这一下多半是有目的的,真正想要的一般是
 *  **已经存的那篇**,所以第一按钮是跳过去。
 *
 *  「仍然存一份」不能省:文章更新了想重存是合理需求,判重一旦只进不出,
 *  用户就只剩"自己去剪藏目录里改文件名"这一条路——那不是防重复,
 *  那是把活推给用户。 */
function announceDuplicate(filename: string, capture: ClipboardCapture): void {
  showToast(t("toast.duplicate"), [
    { label: t("toast.duplicateOpen"), primary: true, onClick: () => void openDetail(filename) },
    {
      label: t("toast.duplicateForce"),
      onClick: () => {
        hideToast();
        void saveCapture(capture, true)
          .then((again) => {
            // 判重是后端说了算,带着 force 仍被判重说明中间有人动过库
            if (again) announceDuplicate(again, capture);
            else showToast(t("toast.saved"));
          })
          .catch((err) => showError(wireText(err)));
      },
    },
    { label: t("toast.close"), onClick: hideToast },
  ]);
}

async function pasteNow(): Promise<void> {
  try {
    const capture = await api.captureClipboard();
    if (!capture.html?.trim() && !capture.text.trim()) {
      showError(t("error.clipboardEmpty"));
      return;
    }
    const duplicate = await saveCapture(capture);
    if (duplicate) announceDuplicate(duplicate, capture);
    else showToast(t("toast.saved"));
  } catch (err) {
    showError(wireText(err));
  }
}

async function runSearch(query: string): Promise<void> {
  const trimmed = query.trim();
  if (!trimmed) {
    // 清空搜索框 = 回到全库列表,不是"搜了个空词"
    hits = null;
    renderList();
    return;
  }
  try {
    // 搜索跟着当前视图走。人在回收站里搜,搜的却是**库里**的东西的话,
    // 他会以为"我明明删了它怎么还搜得到"——那等于删了个寂寞
    const inTrash = filter === "trash";
    const results = inTrash
      ? await api.searchTrash(trimmed)
      : await api.searchClips(trimmed);
    // 用户可能已经改词、换视图或清空了。这次的返回值过期,丢掉
    if (searchEl.value.trim() !== trimmed || filter === "trash" !== inTrash) return;
    hits = results;
    renderList();
  } catch (err) {
    showBackendError("error.searchFailed", err);
  }
}

// 每敲一下就全库扫一遍,剪藏多了会跟着手抖。去抖 200ms 是体感不明显的下限。
function onQueryChanged(): void {
  if (searchTimer) clearTimeout(searchTimer);
  const value = searchEl.value;
  searchTimer = setTimeout(() => void runSearch(value), 200);
}

// **两个事件都得听。** `input[type=search]` 自带一套清除行为:按 Esc 或者
// 点框里的 × ,Chromium 会把值清空,但只抛 `search` 和 `change`,**不抛
// `input`**。只听 input 的话,用户按一下 Esc 退出搜索框,值是空了、列表却
// 永远停在"搜到 0 条"——看上去就像剪藏全没了。
searchEl.addEventListener("input", onQueryChanged);
searchEl.addEventListener("search", onQueryChanged);

async function refreshList(): Promise<void> {
  try {
    const result = await api.listClips();
    clips = result.clips;

    if (result.unreadable.length > 0) {
      // 这些文件确实存在但读不出元数据。藏起来等于骗用户"剪藏丢了",
      // 摆出来用户自己能看到是哪个文件出了问题
      showError(
        t("error.unreadableFiles", {
          n: result.unreadable.length,
          names: result.unreadable.map((f) => f.filename).join("、"),
        }),
      );
    } else {
      clearError();
    }

    // 回收站开着的时候,库的变化也得让回收站跟着重算一遍——
    // 撤销、别的窗口删东西,都走这条路
    if (filter === "trash") await refreshTrash();
    await refreshTags();
    renderList();
  } catch (err) {
    showBackendError("error.listFailed", err);
  }
}

async function loadVaultInfo(): Promise<void> {
  try {
    const info: VaultInfo = await api.vaultInfo();
    vaultPathEl.textContent = info.path;
    vaultPathEl.title = info.path;
    watchEl.checked = info.watching;
  } catch {
    vaultPathEl.textContent = t("error.vaultUnknown");
  }
}

el<HTMLButtonElement>("btn-paste").addEventListener("click", () => void pasteNow());

el<HTMLButtonElement>("btn-open").addEventListener("click", () => {
  void api
    .openVaultFolder()
    .catch((e) => showError(wireText(e)));
});

/** 切筛选。搜索态下不切——搜出来的结果和自己的筛选无关,
 *  硬切会让用户以为"搜到的东西被筛没了",是搜索坏了。 */
async function setFilter(next: ListMode | "trash"): Promise<void> {
  // 搜索结果和选区都跟着上一个视图。不清掉的话,用户会在回收站里
  // 看见**库里**搜出来的结果,或者对着一个已经看不见的选区按删除
  hits = null;
  selected = new Set();
  selectionAnchor = null;
  searchEl.value = "";
  filter = next;
  for (const [button, value] of [
    [filterAllEl, "all"],
    [filterUnreadEl, "unread"],
    [filterWeekEl, "week"],
    [filterArchivedEl, "archived"],
    [filterTrashEl, "trash"],
  ] as const) {
    const on = value === next;
    button.classList.toggle("active", on);
    button.setAttribute("aria-selected", String(on));
  }
  if (next === "trash") {
    // 回收站里没有标签这回事,带着标签筛选进去的话,出来之后列表是空的,
    // 用户会以为标签把剪藏弄丢了
    tagFilter = null;
    try {
      await refreshTrash();
    } catch (err) {
      showBackendError("error.trashUnreadable", err);
    }
  }
  renderTagBar();
  renderList();
  // 换视图之后原来那篇多半不在新列表里了,详情页得跟着换,
  // 否则会停在一篇"看得见却不在列表中"的文章上
  if (activeFilename && !visibleFilenames().includes(activeFilename)) {
    activeFilename = null;
    renderEmptyDetail();
  }
}
filterAllEl.addEventListener("click", () => void setFilter("all"));
filterUnreadEl.addEventListener("click", () => void setFilter("unread"));
filterWeekEl.addEventListener("click", () => void setFilter("week"));
filterArchivedEl.addEventListener("click", () => void setFilter("archived"));
filterTrashEl.addEventListener("click", () => void setFilter("trash"));

/** 清空是不可逆的,而且一次动的是**全部**。主按钮写成"取消"——
 * 要用户多点一下才够得到那个红色的,总比反过来安全。 */
function askEmptyTrash(): void {
  if (trash.length === 0) return;
  showToast(t("confirm.emptyTrash", { n: trash.length }), [
    { label: t("batch.clear"), primary: true, onClick: hideToast },
    { label: t("confirm.emptyTrashAction"), onClick: () => void doEmptyTrash() },
  ]);
}

async function doEmptyTrash(): Promise<void> {
  hideToast();
  try {
    const report = await api.emptyTrash();
    // 失败的还在回收站里,得留在列表上,不能一股脑清空本地状态
    const failed = new Set(report.failed.map((f) => f.filename));
    trash = trash.filter((t) => failed.has(t.filename));
    activeFilename = null;
    renderTrashList();
    renderEmptyDetail();
    if (report.failed.length > 0) {
      // "清空"没清干净必须说出来。报一句成功了,用户会以为磁盘已经腾干净了
      showError(
        t("error.emptyTrashPartial", {
          ok: report.succeeded.length,
          n: report.failed.length,
          names: report.failed.map((f) => f.filename).join("、"),
        }),
      );
    } else {
      showToast(t("toast.clearedTrash", { n: report.succeeded.length }));
    }
  } catch (err) {
    showBackendError("error.emptyTrashFailed", err);
  }
}

/* ── 批量操作 ── */

function selectedFiles(): string[] {
  return [...selected];
}

/** 一次改多篇的标志。**逐条走 `set_flags` 的同一条路**,所以"不许弄坏
 * 用户文件"那两条规矩在批量下照样成立。 */
async function batchFlags(read: boolean | undefined, archived: boolean | undefined, what: string): Promise<void> {
  const names = selectedFiles();
  if (names.length === 0) return;
  try {
    const report = await api.setClipFlagsBatch(names, read, archived);
    const done = new Set(report.succeeded);
    // 成功的那些从选区里去掉,失败的留着——用户看得见"哪几篇没成",
    // 可以再点一次重试,而不是操作完还得回头猜是哪几篇
    selected = new Set([...selected].filter((f) => done.has(f)));
    // 批量报告只带回文件名,不带改完的摘要。与其在内存里把标志拼回去
    // (拼错了就是"界面说已读、文件里还是未读"),不如老实重扫一遍磁盘
    await refreshList();
    if (activeFilename) await refreshOpenDetail();
    reportBatch(report, t("batch.count", { n: report.succeeded.length }));
  } catch (err) {
    showBackendError("error.batchFailed", err, { action: what });
  }
}

function reportBatch(report: BatchReport, done: string): void {
  if (report.failed.length === 0) {
    showToast(done);
    return;
  }
  // 批量最容易出的事就是"悄悄少做了一半"。只报成功那几条的话,
  // 用户会以为没做的那几篇也做了
  showError(
    t("error.batchPartial", {
      done,
      n: report.failed.length,
      reason: wireText(report.failed[0].reason),
    }),
  );
}

el<HTMLButtonElement>("batch-read").addEventListener("click", () =>
  void batchFlags(true, undefined, t("batch.read")),
);
el<HTMLButtonElement>("batch-unread").addEventListener("click", () =>
  void batchFlags(false, undefined, t("batch.unread")),
);
el<HTMLButtonElement>("batch-archive").addEventListener("click", () =>
  void batchFlags(undefined, true, t("batch.archive")),
);
el<HTMLButtonElement>("batch-clear").addEventListener("click", () => {
  selected = new Set();
  selectionAnchor = null;
  renderList();
});

el<HTMLButtonElement>("batch-delete").addEventListener("click", () => {
  const names = selectedFiles();
  // 删除是不可逆的(要进回收站才能撤销),而且一次动的是全部——
  // 主按钮写成"取消",要用户多点一下才够得到那个红色的
  showToast(t("confirm.batchDelete", { n: names.length }), [
    { label: t("batch.clear"), primary: true, onClick: hideToast },
    {
      label: t("confirm.batchDeleteAction"),
      onClick: () => {
        hideToast();
        void trashMany(names)
          .then((report) =>
            reportBatch(report, t("toast.movedMany", { n: report.succeeded.length })),
          )
          .catch((err) => showError(wireText(err)));
      },
    },
  ]);
});

el<HTMLButtonElement>("btn-import").addEventListener("click", async () => {
  try {
    const { report } = await api.importMarkdown();
    await refreshList();
    reportImport(report);
  } catch (err) {
    // 用户在目录选择器上点了取消,那不是故障。**按代号判,不按中文句子判**:
    // 拿 `String(err).includes("已取消")` 判,文案一改就失效,
    // 而且英文界面下那句中文压根不会出现,取消会被当成真错误弹红字
    if (isWireError(err) && err.code === "app.cancelled") return;
    showBackendError("error.importFailed", err);
  }
});

/** 导入结果。**跳过的那些必须列出来**:用户导进一个存过一堆旧文的文件夹,
 * 里面有一半是重复的,界面只报一句"导入完成"的话,他没法判断到底进来了多少。 */
function reportImport(report: BatchReport): void {
  const n = report.succeeded.length;
  if (n === 0) {
    showError(
      report.failed.length > 0
        ? t("error.importNone", {
            n: report.failed.length,
            name: report.failed[0].filename,
            reason: wireText(report.failed[0].reason),
          })
        : t("error.importNoneEmpty"),
    );
    return;
  }
  if (report.failed.length === 0) {
    showToast(t("toast.imported", { n }));
    return;
  }
  showError(
    t("error.importPartial", {
      ok: n,
      n: report.failed.length,
      names:
        report.failed
          .slice(0, 3)
          .map((f) => `${f.filename}(${wireText(f.reason)})`)
          .join("、") + (report.failed.length > 3 ? ` 等 ${report.failed.length} 个` : ""),
    }),
  );
}

el<HTMLButtonElement>("btn-export").addEventListener("click", async () => {
  try {
    // 返回 null 是用户在保存对话框点了取消,那不是故障,别弹红字
    const path = await api.exportVault();
    if (path) showToast(t("toast.exported"));
  } catch (err) {
    showBackendError("error.exportFailed", err);
  }
});

el<HTMLButtonElement>("btn-pick").addEventListener("click", async () => {
  try {
    const info = await api.pickVault();
    if (info) {
      await loadVaultInfo();
      await refreshList();
    }
  } catch (err) {
    showBackendError("error.pickVaultFailed", err);
  }
});

watchEl.addEventListener("change", () => {
  void api
    .setClipboardWatch(watchEl.checked)
    .then((info) => {
      watchEl.checked = info.watching;
      if (info.watching) {
        showToast(t("toast.watchOn"));
      } else {
        hideToast();
      }
    })
    .catch((err) => {
      watchEl.checked = !watchEl.checked; // 状态没改成,把开关拨回去
      showError(wireText(err));
    });
});

/** 用户正在打字的话就别抢键。搜索框里敲 r 是搜索 r,不是标已读。 */
function isTyping(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el) return false;
  return el.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(el.tagName);
}

/** 处理完一篇之后接着读下一篇。
 *
 *  标了已读/归档之后,那一篇多半已经从当前视图里消失了(`r` 在「未读」里
 *  按一下就消失)。光按文件名找不到它了,得用**改动前记下的下标**找它下面
 *  的那篇,不然用户会退回第一篇重读一遍。 */
function advanceAfterRemoval(indexBefore: number): void {
  const next = reanchorAfterRemoval(visibleFilenames(), indexBefore);
  if (next) {
    void openDetail(next);
  } else {
    activeFilename = null;
    renderEmptyDetail();
    renderList();
  }
}

document.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "v") {
    // Ctrl+V 走的是系统剪贴板,不是往 DOM 里插文本,所以要拦下默认行为
    e.preventDefault();
    void pasteNow();
    return;
  }

  if (e.key === "Escape") {
    if (isTyping(e.target)) {
      (e.target as HTMLElement).blur();
      return;
    }
    // 选着东西的时候按 Esc 是"取消选择",不是关窗口
    if (selected.size > 0) {
      selected = new Set();
      selectionAnchor = null;
      renderList();
      return;
    }
  }

  // 全选只选**看得见的**。看不见的跟着一起删掉,那是制造事故
  if ((e.ctrlKey || e.metaKey) && !e.shiftKey && e.key.toLowerCase() === "a" && !isTyping(e.target)) {
    if (filter !== "trash") {
      e.preventDefault();
      selected = selectAll(visibleFilenames());
      selectionAnchor = null;
      renderList();
    }
    return;
  }
  if (e.key === "/" && !isTyping(e.target)) {
    e.preventDefault();
    searchEl.focus();
    searchEl.select();
    return;
  }
  if (e.ctrlKey || e.metaKey || e.altKey || isTyping(e.target)) return;

  const key = e.key.toLowerCase();
  if (key === "arrowdown" || key === "j") {
    e.preventDefault();
    const next = moveSelection(visibleFilenames(), activeFilename, 1);
    // 移动就直接打开:稍后读是拿来"一篇篇读过去"的,让人多按一下回车
    // 只会把连着读变成点读
    if (next) void (filter === "trash" ? openTrashDetail(next) : openDetail(next));
    return;
  }
  if (key === "arrowup" || key === "k") {
    e.preventDefault();
    const prev = moveSelection(visibleFilenames(), activeFilename, -1);
    if (prev) void (filter === "trash" ? openTrashDetail(prev) : openDetail(prev));
    return;
  }

  // 回收站里 r / a 说的全是"库里那篇",在这里按下去会去改一篇
  // 根本不在列表里的剪藏——看起来像按了没反应,其实是按错了地方
  if (filter === "trash") return;

  const current = activeFilename ? clips.find((c) => c.filename === activeFilename) : null;
  if (!current) return;
  if (key === "r" || key === "a") {
    e.preventDefault();
    // 下标必须在动手**之前**记。动完之后这一篇已经不在列表里了,
    // 那时候再找它的位置只会得到 -1,接着读就退回第一篇重来了
    const at = visibleFilenames().indexOf(current.filename);
    const done = key === "r" ? toggleRead(current, !current.read) : toggleArchive(current.filename);
    void done.then(() => {
      // 还留在列表里(比如在「全部」视图里标已读)就别乱动光标,
      // 用户可能还想把它归档
      if (!visibleFilenames().includes(current.filename)) advanceAfterRemoval(at);
    });
  }
});

el<HTMLButtonElement>("toast-close").addEventListener("click", hideToast);

/** 换语言。**不是刷新一遍**:正开着的那篇正文得留着,滚动位置也得留着——
 *  为了改个语言把用户读到的位置弹回顶部,下次他就不切了。 */
function applyLocale(): void {
  applyI18n(root);
  langBtn.textContent = localeName();
  langBtn.title = t("lang.switch");
  langBtn.setAttribute("aria-label", t("lang.switch"));
  hideToast();
  clearError();
  void reloadAfterLocale();
}

async function reloadAfterLocale(): Promise<void> {
  if (filter === "trash") {
    try {
      await refreshTrash();
    } catch (err) {
      showBackendError("error.trashUnreadable", err);
    }
  }
  renderList();
  if (activeFilename) {
    try {
      const clip = await (filter === "trash"
        ? api.readTrashClip(activeFilename)
        : api.readClip(activeFilename));
      // 换语言要花一会儿,用户可能已经点走别的了
      if (!activeFilename) return;
      if (filter === "trash") renderTrashDetail(clip);
      else renderDetail(clip);
    } catch (err) {
      if (filter !== "trash") showBackendError("error.readFailed", err);
    }
  }
}

el<HTMLButtonElement>("btn-lang").addEventListener("click", () => {
  setLocale(otherLocale());
  applyLocale();
});
async function boot(): Promise<void> {
  // 先挂监听再拉列表。反过来的话,在这两步之间发生的剪藏不会触发任何事件,
  // 用户会看到"扩展显示剪藏成功,列表里却没有"
  await listen("clip-saved", () => void refreshList());
  await listen("vault-changed", () => void refreshList());
  // 图片是后台下的,下完了才通知。这条提示是**特意要说出来的**:
  // Quire 一直说自己不联网,现在剪藏这一刻会真的去连图片服务器,
  // 悄悄做和写在脸上是两回事
  await listen<{ filename: string; count: number }>("clip-saved-images", (e) => {
    showToast(t("toast.savedImages", { count: e.payload.count }));
  });
  await listen<ClipboardCapture>("clipboard-changed", (event) => {
    pendingCapture = event.payload;
    const preview =
      event.payload.text?.trim().split("\n").find((l) => l.trim())?.slice(0, 40) ||
      t("toast.detectedFallback");
    showToast(t("toast.detected", { preview }), [
      {
        label: t("toast.save"),
        primary: true,
        onClick: () => {
          const capture = pendingCapture;
          hideToast();
          if (!capture) return;
          void saveCapture(capture)
            .then((duplicate) => {
              if (duplicate) announceDuplicate(duplicate, capture);
              else showToast(t("toast.saved"));
            })
            .catch((err) => showError(wireText(err)));
        },
      },
      { label: t("toast.ignore"), onClick: hideToast },
    ]);
  });

  await loadVaultInfo();
  await refreshList();
  renderTagBar();
  renderEmptyDetail();
}

void boot();
