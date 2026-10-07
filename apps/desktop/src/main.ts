import "./style.css";
import { listen } from "@tauri-apps/api/event";
import { convertFileSrc } from "@tauri-apps/api/core";
import { api } from "./api";
import type { ClipboardCapture } from "./clipboard";
import {
  applyI18n,
  isWireError,
  localeName,
  otherLocale,
  setLocale,
  t,
  toWireError,
  wireText,
} from "./i18n";
import { restorable, snapshotClips, undoBatch } from "./batch-undo";
import type { BatchUndo, ClipSnapshot } from "./batch-undo";
import { clipItem, formatWhen } from "./clip-item";
import type { ClipItemContext } from "./clip-item";
import { openContextMenuAt } from "./context-menu";
import type { MenuItem } from "./context-menu";
import { SHORTCUTS } from "./shortcuts";
import { libraryStats } from "./stats";
import { clipToMarkdown, MARKDOWN_PLACEHOLDER, renderMarkdown, resolveAssetImages } from "./markdown";
import { startRename as renameTitle } from "./rename";
import { attachNoteEditor } from "./note";
import { afterRefresh } from "./refresh";
import { undoableFiles, undoTrashMany as undeleteApi } from "./undelete";
import type { UndeleteResult } from "./undelete";
import { createProgressJudge, createProgressQueue } from "./progress";
import {
  groupByWeek,
  moveSelection,
  pruneSelection,
  reanchorAfterRemoval,
  LIST_PAGE,
  listWindow,
  selectAll,
  selectClips,
  selectRange,
  selectByTag,
  toggleSelected,
  unreadCount,
  upsertClip,
  withTag,
  withoutTag,
  type ListMode,
  type Selection,
} from "./list";
import * as clipHistory from "./clipboard-history";
import { plain } from "./highlight";
import type {
  BatchReport,
  ClipContent,
  DataImportReport,
  LibraryStatus,
  ClipSummary,
  SearchHit,
  SearchScope,
  TrashItem,
  VaultInfo,
  VaultMigrated,
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
    <label class="search-scope">
      <span class="visually-hidden" data-i18n="search.scope.label"></span>
      <select id="search-scope" data-i18n-aria="search.scope.label">
        <option value="any" data-i18n="search.scope.any"></option>
        <option value="title" data-i18n="search.scope.title"></option>
        <option value="body" data-i18n="search.scope.body"></option>
        <option value="tag" data-i18n="search.scope.tag"></option>
        <option value="note" data-i18n="search.scope.note"></option>
      </select>
    </label>
    <div class="filters" role="tablist" data-i18n-aria="toolbar.filters.aria">
      <button class="filter active" id="filter-all" role="tab" aria-selected="true" data-i18n="filter.all"></button>
      <button class="filter" id="filter-today" role="tab" aria-selected="false" data-i18n="filter.today" data-i18n-title="filter.today.title"></button>
      <button class="filter" id="filter-unread" role="tab" aria-selected="false" data-i18n="filter.unread" data-i18n-title="filter.unread.title"></button>
      <button class="filter" id="filter-week" role="tab" aria-selected="false" data-i18n="filter.week" data-i18n-title="filter.week.title"></button>
      <button class="filter" id="filter-starred" role="tab" aria-selected="false" data-i18n="filter.starred" data-i18n-title="filter.starred.title"></button>
      <button class="filter" id="filter-archived" role="tab" aria-selected="false" data-i18n="filter.archived" data-i18n-title="filter.archived.title"></button>
      <button class="filter" id="filter-trash" role="tab" aria-selected="false" data-i18n="filter.trash" data-i18n-title="filter.trash.title"></button>
    </div>
    <!-- 不挂 data-i18n:那个键带 {n} 占位符,静态渲染出来会带着花括号。文案由 updateHistoryBadge 填 -->
    <button class="btn ghost" id="btn-history" hidden></button>
    <!-- 不挂 data-i18n:这个按钮只在真有欠账时才出现,文案带 {n} 占位符,静态渲染会带着花括号 -->
    <button class="btn ghost" id="btn-status" hidden></button>
    <button class="btn ghost" id="btn-stats" data-i18n="toolbar.stats" data-i18n-title="toolbar.stats.title"></button>
    <button class="btn" id="btn-import" data-i18n="toolbar.import" data-i18n-title="toolbar.import.title"></button>
    <button class="btn" id="btn-export" data-i18n="toolbar.export" data-i18n-title="toolbar.export.title"></button>
    <button class="btn" id="btn-open" data-i18n="toolbar.open"></button>
    <button class="btn" id="btn-refresh" data-i18n="toolbar.refresh" data-i18n-title="toolbar.refresh.title"></button>
    <!-- 设置。剪贴板监控、界面语言、数据目录都在里面。工具栏只留
         「每天都要按」的那些:剪藏、搜索、筛、导入导出、刷新 -->
    <button class="btn ghost" id="btn-settings" data-i18n="toolbar.settings" data-i18n-title="toolbar.settings.title"></button>
    <!-- 这一行既是提示也是入口。**做成能点的**:不认识快捷键的人不会去按
         一个不认识的键,而认得几个的人想知道"还有没有别的"时,总得有个地方点 -->
    <button
      type="button"
      class="shortcut-hint"
      id="btn-help"
      data-i18n-title="toolbar.shortcut.title"
      data-i18n-html="toolbar.shortcut.text"
    ></button>
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
        <button class="btn" id="batch-star" data-i18n="batch.star" data-i18n-title="batch.star.title"></button>
        <button class="btn" id="batch-tags" data-i18n="batch.tags"></button>
        <button class="btn" id="batch-export" data-i18n="batch.export" data-i18n-title="batch.export.title"></button>
        <button class="btn danger" id="batch-delete" data-i18n="batch.delete"></button>
        <button class="btn ghost" id="batch-clear" data-i18n="batch.clear"></button>
      </div>
      <div class="batch-tag-panel" id="batch-tag-panel" hidden></div>
    </aside>
    <section class="detail-side">
      <!-- 阅读进度是栏左边缘的竖线:**不进滚动内容**。放进内容流里的话,
           要么靠负 margin 硬顶(横在标题区中间),要么 sticky(滚动时压住
           穿过来的正文),两版都翻过车。竖线在栏边缘的 3px 里,正文左侧
           留白 40px,永远碰不着;读多少长多少,方向和滚动同向 -->
      <div class="read-progress" id="read-progress" data-i18n-title="detail.progress.title"><i id="read-progress-bar"></i></div>
      <section class="detail-pane" id="detail"></section>
    </section>
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
const batchTagPanelEl = el<HTMLDivElement>("batch-tag-panel");
const detailEl = el<HTMLElement>("detail");
const progressBarEl = el<HTMLElement>("read-progress-bar");
const warnEl = el<HTMLDivElement>("warn");
const tagBarEl = el<HTMLDivElement>("tag-bar");
const vaultPathEl = el<HTMLSpanElement>("vault-path");
// 当前剪藏库的根路径。本地化过的图片地址是相对路径,渲染时要拼成
// 绝对路径再转 asset 地址;库没就绪(空态/报错)时是空串
let vaultRoot = "";

/** 正文里相对地址的基准目录:**md 所在的 clips 目录,不是库根**。
 *  图片本地化把图下到 `<库>/clips/assets/<id>/`、正文里写 `assets/...`,
 *  这个相对路径对 md 文件本身成立(任何 md 阅读器都能显示),
 *  Quire 渲染时也得照同一个基准拼——拿库根拼的话永远 404 裂图。
 *  `CLIPS_DIR` 在 Rust 侧 vault.rs,改名要两头一起。 */
function clipsBase(): string {
  const root = vaultRoot.trim().replace(/[\\/]+$/, "");
  return root ? `${root}/clips` : "";
}
const toastEl = el<HTMLDivElement>("toast");
const toastTextEl = el<HTMLSpanElement>("toast-text");
const toastActionsEl = el<HTMLSpanElement>("toast-actions");
// 剪贴板监控的当前状态。开关本体在设置面板里(每次打开重新构建),
// 状态得存在模块这一层——多处逻辑要读它,存 DOM 上不方便
let watchOn = false;

/** **问过没有。** 空库那一屏上摆不摆「要不要开启」全看它——
 *  用户拒绝过一次之后再问,那是骚扰不是引导 */
let watchAsked = false;
const searchEl = el<HTMLInputElement>("search");
const filterAllEl = el<HTMLButtonElement>("filter-all");
const filterTodayEl = el<HTMLButtonElement>("filter-today");
const filterUnreadEl = el<HTMLButtonElement>("filter-unread");
const filterWeekEl = el<HTMLButtonElement>("filter-week");
const filterStarredEl = el<HTMLButtonElement>("filter-starred");
const filterArchivedEl = el<HTMLButtonElement>("filter-archived");
const filterTrashEl = el<HTMLButtonElement>("filter-trash");
const settingsBtnEl = el<HTMLButtonElement>("btn-settings");
let settingsPanelEl: HTMLElement | null = null;
const refreshBtn = el<HTMLButtonElement>("btn-refresh");

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
/** 还没写出去的进度。**跟定时器分开存**,切文章时定时器会被摘掉,
 *  值得另找地方放——不然那一次就跟着定时器一起没了。 */
const progressQueue = createProgressQueue();
/** 存盘提示的判官:同一场故障只报一次,好了再补一句把事结掉。 */
const progressJudge = createProgressJudge();
/** Shift 连选的锚点。`null` 表示还没点过起点——这时按 Shift 只选当前那条。 */
let selectionAnchor: string | null = null;
/** 批量标签面板开着没有。**不做成弹窗**:弹窗会把列表整个盖住,
 * 用户勾着标签想看看筛出来哪些,盖住了就没法看。展开在批量条下面,
 * 列表还在,勾完当场就能看见结果。 */
let batchTagPanelOpen = false;
/** 面板里勾中的标签。空数组 = 勾了零个(和"面板没开"是两回事)。 */
let batchTagsPicked: string[] = [];
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
/** 详情页读到的是**哪一份**的指纹。写回去时带上,后端写前比一次。
 *
 *  没有它就有一个静悄悄吃数据的场景:你打开一篇写了半句批注(还没失焦、
 *  还在输入框里),这中间去 Obsidian 改了正文,回来接着写、失焦自动存——
 *  `setClipNote` 读的是**磁盘上此刻的文件**,它自己不知道输入框里的内容
 *  是几秒前的。丢的不只是那半句批注,**正文也会一起被写回成旧的那份**
 *
 *  只认读详情时后端给的那一份。写成功了就得重新读一次拿新的指纹——
 *  不然自己刚写完的东西会被自己判成"别处改过",第二次保存就再也过不去 */
let activeStamp: string | null = null;

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

/** 这一篇是不是在别处被改过了。后端只在**指纹对不上**时报这个,
 *  而不是笼统的写失败——用户据此知道自己的字还在输入框里,没被冲掉。 */
function isConflict(err: unknown): boolean {
  return isWireError(err) && err.code === "vault.changedElsewhere";
}

/** 冲突了怎么办:**先停下来,再问要不要覆盖**。
 *
 *  **默认动作是"不写"。** 自动帮用户合并看着像贴心,实际是在他没参与
 *  的情况下决定谁的内容该活下来——而他甚至不知道有冲突发生过
 *
 *  两个选择都得给:
 * - 用我的版本覆盖回去——他确实知道自己写的是什么(比如刚在 Obsidian
 *   里重排了格式,批注还是要加上),这时候拦他只是添乱
 * - 丢掉我的改动——他把磁盘上那份最新的读回来接着改,这通常是更常见的
 *   那一半,所以放在前面、做成主按钮
 *
 *  `retry` 是**不带指纹**重试一次:带了就是拿几秒前那份盖掉刚敲的,
 *  那正是这个守卫要拦的事。冲突得由用户显式解除,不能自动解除
 */
function reportConflict(retry: () => Promise<void>): void {
  showError(t("error.conflict"));
  showToast(t("toast.conflict"), [
    { label: t("toast.conflict.discard"), onClick: () => void openDetailNow() },
    {
      label: t("toast.conflict.overwrite"),
      primary: true,
      onClick: () => {
        hideToast();
        void retry();
      },
    },
  ]);
}

/** 重新读一遍当前这篇,把详情页和指纹一起刷成磁盘上最新的那份。
 *
 *  冲突之后走这条路。**顺带把输入框重画一遍**,用户那半句批注会没——
 *  所以提示里得说清楚,不然后面字没了用户会以为软件在偷他的东西 */
async function openDetailNow(): Promise<void> {
  if (!activeFilename) return;
  const filename = activeFilename;
  try {
    const clip = await api.readClip(filename);
    if (activeFilename !== filename) return;
    activeStamp = clip.stamp ?? null;
    renderDetail(clip);
    clearError();
  } catch (err) {
    showBackendError("error.readFailed", err);
  }
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
  renderUnreadBadge();
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

/** 「未读」按钮上写个数字。
 *
 * **为什么不弹通知、不做每日回顾。** 那是「推」,用户没要求就被推着走,
 * 用两次就烦了,烦了就关掉推送,再往后连开都不开了。这个数字是「拉」:
 * 打开 Quire 的时候自己看见「你还有 8 篇没看」,要不要看是他的决定。
 * 差别在于**打开的理由**——没有它,打开 Quire 的理由只有「我又想存点东西」;
 * 有了它,还有「我欠了自己 8 篇」
 *
 * **0 就不写。** 挂个 0 只是让人多看一眼,而「0」本身不传递任何信息
 */
function renderUnreadBadge(): void {
  // 搜索中不写。**搜索框里那几篇是搜出来的,不是"全部"**——
  // 用户搜「苹果」看到「未读 3」,会以为库里只剩 3 篇没读
  if (hits) {
    filterUnreadEl.textContent = t("filter.unread");
    filterUnreadEl.removeAttribute("title");
    filterUnreadEl.removeAttribute("aria-label");
    return;
  }
  const n = unreadCount(clips);
  if (n === 0) {
    filterUnreadEl.textContent = t("filter.unread");
    filterUnreadEl.removeAttribute("title");
    filterUnreadEl.removeAttribute("aria-label");
    return;
  }
  const label = t("list.unreadBadge", { n });
  filterUnreadEl.textContent = `${t("filter.unread")} ${label}`;
  const full = `${t("filter.unread.title")} · ${label}`;
  filterUnreadEl.title = full;
  filterUnreadEl.setAttribute("aria-label", full);
}

/** 「补全全文」按钮。**没有可抓的地址就不摆。**
 *
 * 剪藏那一刻网页抓不到、只存了用户复制的那一小段,几周后他想起那篇文章想看
 * 全文——那时候只剩这一条路。旧版本里他只能删了重剪一遍,而重剪多半还是失败
 *
 * **用同一个阈值判断"当时有没有抓过"**:正文短于 `FULL_TEXT_MIN_CLIP_CHARS`
 * 就说明当初没抓到全文。用同一个数而不是另定一个,是为了让"当初为什么没抓"
 * 和"现在能不能补"永远说同一件事
 */
function buildRefetchButton(clip: ClipSummary, bodyEl: HTMLElement): HTMLButtonElement | null {
  const url = clip.url?.trim();
  if (!url || !/^https?:\/\//i.test(url)) return null;
  const len = (bodyEl.textContent ?? "").trim().length;
  if (len >= FULL_TEXT_MIN_CLIP_CHARS) return null;

  const btn = document.createElement("button");
  btn.className = "btn ghost";
  btn.textContent = t("detail.refetch");
  btn.title = t("detail.refetch.title");
  btn.addEventListener("click", () => void refetchFullText(clip, btn));
  return btn;
}

/** 重新去把整篇抓回来。
 *
 * **抓取在 Rust、抽取在前端,跟剪藏那一步同一条路。** 一次网络往返只发一次,
 * 抓不到就照实说,不重试——那会变成一个在用户眼皮底下反复发请求的东西
 */
async function refetchFullText(clip: ClipSummary, btn: HTMLButtonElement): Promise<void> {
  const url = clip.url?.trim();
  if (!url) return;
  const label = btn.textContent;
  // 禁用不是装饰:**抓取要几秒**,这段时间里用户完全可以再点一次,
  // 于是同一个页面被发两次请求
  btn.disabled = true;
  btn.textContent = t("detail.refetch.running");
  /** 抽出来的正文。**提到 try 外面**,因为冲突之后那次「覆盖重试」
   *  还得用它——留在块里的话重试回调只能看见一个 undefined,
   *  而给 `setClipBody` 传空串等于把这一篇正文清空,那是净损失 */
  let markdown = "";
  /** 真写盘。**指纹是参数不是闭包里的全局**——冲突重试要的是"别查了,
   *  我知道自己在盖别人的东西",正好传 `undefined` */
  const write = async (stamp?: string) => {
    await api.setClipBody(clip.filename, markdown, stamp);
    clearError();
    showToast(t("toast.refetchDone"));
    await openDetail(clip.filename);
  };
  try {
    const page = await api.fetchArticle(url);
    if (!page.html.trim()) {
      showToast(t("toast.refetchEmpty"));
      return;
    }
    // **走 `clipToMarkdown` 那条已经测过的路**,跟剪藏那一刻一模一样。
    // 另开一条抽取路径等于给同一件事写两份实现,迟早不一致——
    // 而用户看到的现象是"重试补出来的正文跟剪藏时长得不一样"
    markdown = clipToMarkdown({
      text: "",
      html: page.html,
      url: page.url || url,
      meta: {},
    }).markdown;
    if (!markdown.trim()) {
      showToast(t("toast.refetchEmpty"));
      return;
    }
    await write(activeStamp ?? undefined);
  } catch (err) {
    // **补全文是最容易吃数据的一步**:它把整篇正文换掉,而用户在这几秒里
    // 完全可能在别的编辑器里改过同一篇。冲突了就停下来问,别默默盖上去
    if (isConflict(err)) reportConflict(() => write(undefined));
    else {
      // **抓不到是正常结局,不是故障。** 需要登录的、纯 JS 渲染的、被拦的,
      // 都属于这一类——那篇用户已经复制到手的片段还在,一个字都没少
      showToast(t("toast.refetchFailed"));
    }
    void err;
  } finally {
    btn.disabled = false;
    btn.textContent = label;
  }
}

/** 一屏先画多少条、现在放到了多少。**它是状态不是常量**——
 *  「显示更多」点的就是它。 */
let listLimit = LIST_PAGE;

/** 换视图时把"放到了多少"拨回去。
 *
 *  **忘了拨就是一个说不清的界面**:用户在上一个视图里点过两次
 *  「显示更多」,切过去第一眼看到的是一个已经翻了两屏的列表,
 *  而他根本没在这个视图里翻过
 */
function resetListWindow(): void {
  listLimit = LIST_PAGE;
}

/** 「还有 N 篇」那一条。
 *
 *  **界面必须说得出"还没画完"。** 少了这一条,上限就成了一次静默的
 *  数据丢失:用户明明存了八百篇,列表只列到两百,他会以为剩下的没了——
 *  而那是这个软件最不能被怀疑的一件事
 */
function moreRow(hidden: number): HTMLElement {
  const row = document.createElement("div");
  row.className = "list-more";
  const note = document.createElement("span");
  note.className = "list-more-note";
  note.textContent = t("list.more", { n: hidden });
  const button = document.createElement("button");
  button.type = "button";
  button.className = "btn";
  button.textContent = t("list.more.action", { n: Math.min(hidden, LIST_PAGE) });
  button.addEventListener("click", () => {
    listLimit += LIST_PAGE;
    renderList();
  });
  row.append(note, button);
  return row;
}

/** 按当前上限画一批,底下压着的就摆一条「还有 N 篇」。
 *
 *  **画了什么,键盘和全选就认什么**——那几个功能全靠读 DOM
 *  (`visibleFilenames`)。所以截断和它们是自洽的:看不见的既点不到,
 *  也不会被 Ctrl+A 圈进去拖去删掉
 */
function appendPaged<T>(items: T[], render: (item: T) => HTMLElement): void {
  const { shown, hidden } = listWindow(items, listLimit);
  for (const item of shown) listEl.append(render(item));
  if (hidden > 0) listEl.append(moreRow(hidden));
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
      listEl.append(buildSearchMiss(searchEl.value));
      return;
    }
    renderHitList(hits);
    return;
  }

  if (clips.length === 0) {
    listEl.append(buildEmptyState(filter, tagFilter));
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

  appendPaged(shown, (clip) => clipItem(clip, plain(clip.excerpt), {}, clipCtx));
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
  // **先定"画哪几条",再按周画。** 反过来做的话,要么整周整周地跳过
  // (一周五十篇就吃掉五十个额度),要么得在每个周里各算一次上限——
  // 而"还有 N 篇"要报的是**整个列表**压着多少,不是某一周压着多少
  const ordered: ClipSummary[] = [];
  for (const group of groups) ordered.push(...selectClips(group.clips, "all"));
  const { shown, hidden } = listWindow(ordered, listLimit);
  const keep = new Set(shown.map((c) => c.filename));

  for (const group of groups) {
    const inWeek = selectClips(group.clips, "all").filter((c) => keep.has(c.filename));
    // 这一周一条都没轮上就不画周头。**空周头比没有更坏**:用户看到一个
    // 「共 12 篇」的抬头下面一条都没有,只会以为渲染坏了
    if (inWeek.length === 0) continue;
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
    for (const clip of inWeek) {
      listEl.append(clipItem(clip, plain(clip.excerpt), {}, clipCtx));
    }
  }
  if (hidden > 0) listEl.append(moreRow(hidden));
}

/** 检索态下列表长这样:标题、站点时间,外加一段命中上下文。
 *  正文摘要在这儿没用——用户搜的就是这几个字,得让他看见它们出现在哪儿。 */
/** 搜到了什么。**近似命中排在前面,前面挂一条说明。**
 *
 * 后端只在标准搜索一条都没命中时才走容错,所以正常情况下这里全是标准结果。
 * 但万一哪天真混进来一批,界面上得说清楚"这些是差一个字找到的",
 * 而它们的行里也各带一个标——**两处都说是必须的**:横幅解释的是这一批,
 * 行上的标回答的是"为什么这一条不一样" */
function renderHitList(results: SearchHit[]): void {
  const fuzzyCount = results.filter((h) => h.fuzzy).length;
  if (fuzzyCount > 0) {
    listEl.append(buildFuzzyNotice(fuzzyCount));
  }
  // 搜索结果也过同一道上限。后端本来封了 200 条,但那是**协议**上的闸,
  // 不是画布容量——两边哪天对不上,这里照样会把几千条一次性铺出来
  appendPaged(results, (hit) =>
    clipItem(hit.summary, { text: hit.snippet, marks: hit.marks }, {
      readToggle: filter !== "trash",
      titleMarks: hit.titleMarks,
      fuzzy: hit.fuzzy,
    }, clipCtx),
  );
}

/** 「没找到」那一屏。**不写死一句话就完事。**
 *
 * 这是整个产品最需要帮忙的一刻——用户记得存过东西,想不起叫什么。
 * 摆一句「换个词试试」等于把球踢回给他,而他连该换什么词都不知道。
 * 所以这里摆两条真的能往下走的路:**换个范围**,和**搜你上次搜过的** */
function buildSearchMiss(query: string): HTMLElement {
  const box = document.createElement("div");
  box.className = "empty empty-search-miss";

  const title = document.createElement("p");
  title.className = "empty-title";
  // 把搜的那个词原样摆出来。"搜苹果没找到"比"没找到"有用得多——
  // 前者他能确认自己搜对了没有
  title.textContent = t("list.searchNone.titleWith", { query });
  box.append(title);

  const body = document.createElement("p");
  body.textContent = t("list.searchNone.body");
  box.append(body);

  // **换范围。** 用户把范围限成"只看标题"却搜了个正文里才有的词,
  // 是搜不到的头号原因,而他自己多半没意识到限了范围
  if (searchScope !== "any") {
    const widen = document.createElement("button");
    widen.className = "btn";
    widen.textContent = t("list.searchNone.widenScope");
    widen.addEventListener("click", () => {
      searchScope = "any";
      scopeEl.value = "any";
      void runSearch(query, false);
    });
    box.append(widen);
  }

  // **最近搜过的。** 点一下就是重新搜一遍,不用他回忆那个词怎么打的
  const usable = recentSearches.filter((q) => q !== query.trim()).slice(0, 5);
  if (usable.length > 0) {
    const recent = document.createElement("div");
    recent.className = "search-miss-recent";
    const label = document.createElement("p");
    label.className = "search-miss-recent-label";
    label.textContent = t("list.searchNone.recent");
    recent.append(label);
    for (const q of usable) {
      const chip = document.createElement("button");
      chip.className = "search-miss-chip";
      chip.textContent = q;
      chip.addEventListener("click", () => {
        searchEl.value = q;
        void runSearch(q, false);
      });
      recent.append(chip);
    }
    box.append(recent);
  }

  return box;
}

function buildFuzzyNotice(count: number): HTMLElement {
  const notice = document.createElement("div");
  notice.className = "fuzzy-notice";
  notice.textContent = t("list.fuzzyNotice", { n: count });
  return notice;
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
  // 总量只累加量得出来的。量不出来的**不静默当 0**——那样用户看到的
  // "共占用 12 MB" 其实是"至少 12 MB",而他没法知道自己被少算了多少
  const known = trash.filter((i) => i.sizeBytes !== null);
  const total = known.reduce((sum, i) => sum + (i.sizeBytes ?? 0), 0);
  const unknown = trash.length - known.length;
  const header = document.createElement("div");
  header.className = "week-header trash-header";
  const label = document.createElement("span");
  label.textContent =
    unknown > 0
      ? t("trash.headerUnknown", {
          count: trash.length,
          size: formatBytes(total),
          n: unknown,
        })
      : t("trash.header", { count: trash.length, size: formatBytes(total) });
  // **列表里混着两个目录,得说明白。** 只写"共 N 篇",用户点进一篇看到"已隔离",
  // 会以为自己记错了或者软件出 bug 了——他不知道回收站和隔离区都被算进了这个 N
  const quarantinedCount = trash.filter((i) => i.quarantined).length;
  const headerNote =
    quarantinedCount > 0
      ? document.createElement("span")
      : null;
  if (headerNote) {
    headerNote.className = "trash-header-note";
    headerNote.textContent = t("trash.quarantineNote", {
      n: quarantinedCount,
      days: quarantineDays,
    });
  }
  // 清空摆在**它作用的东西旁边**,不占工具栏。工具栏是全局的,
  // 在那儿长期摆一个红按钮,用户会怕点错,也把工具栏挤不下了
  const empty = document.createElement("button");
  empty.className = "btn danger";
  empty.textContent = t("trash.emptyTrash");
  empty.title = t("trash.emptyTrash.title");
  empty.addEventListener("click", () => askEmptyTrash());
  header.append(label);
  if (headerNote) header.append(headerNote);
  header.append(empty);
  listEl.append(header);

  // 回收站也会积到几百篇(尤其是隔离区里那些用户没空处理的),
  // 所以走同一道上限。**头上那行"共 N 篇 / 占多少"数的是全部**,
  // 不是画出来的那些——回收站的状态就是"里头总共有多少",不能因为
  // 一屏装不下就把数字改小,那会让用户以为空间被释放了
  appendPaged(trash, (item) => trashItem(item));
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
  // 量不出来就不能编一个。写"0 B"等于告诉他这篇不占地方
  when.textContent = item.summary
    ? formatWhen(item.summary.clippedAt)
    : item.sizeBytes === null
      ? t("trash.sizeUnknown")
      : formatBytes(item.sizeBytes);
  meta.append(site, when);
  // **已隔离的必须单独标出来**。这一条和其他条摆成同一个样子,
  // 用户就没法判断自己还能不能撤回——而"还能不能撤"正是他点进回收站
  // 想确认的第一件事。标签放在标题前面,扫一眼就看得见
  if (item.quarantined) {
    const badge = document.createElement("span");
    badge.className = "trash-quarantined";
    badge.textContent = t("trash.quarantinedBadge");
    badge.title = t("trash.quarantinedBadge.title", { days: quarantineDays });
    meta.prepend(badge);
  }
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

/** 空库时的那一屏。
 *
 *  **这是新用户看到的第一屏,也是最该打磨的一屏。** 装完打开是一片空白,
 *  用户不知道该干什么就关掉了——这不是功能缺失,是缺一句话。
 *
 *  **只给两步。** 不做多步向导、不放教程链接、不弹窗拦路:
 *  那种东西在真正好用的软件里是噪音,用户试了三次没成才会去找「怎么用」。
 *  这里要做的只是把**最不容易自己想到的那一步**说出来——大部分人第一次
 *  用稍后读工具,卡住的都是"到底存哪儿去了",不是"存不了"
 *
 *  走 DOM 不走 innerHTML:标签是用户自己起的,一个 `<img onerror=…>`
 *  塞进去就执行了
 */
function buildEmptyState(mode: ListMode | "trash", tag: string | null): HTMLElement {
  const box = document.createElement("div");
  box.className = "empty";

  // **每种空态说不同的话。** 筛出来的空和库本身就空,含义完全不一样:
  // 用户筛「未读」看到空白,会以为剪藏丢了
  let title: string;
  let body: string;
  if (mode === "trash") {
    title = t("list.trashEmpty.title");
    body = t("list.trashEmpty.body");
  } else if (tag !== null) {
    title = t("list.tagEmpty.title", { tag });
    body = t("list.tagEmpty.body");
  } else if (mode === "unread") {
    title = t("list.unreadEmpty.title");
    body = t("list.unreadEmpty.body");
  } else if (mode === "archived") {
    title = t("list.archivedEmpty.title");
    body = t("list.archivedEmpty.body");
  } else if (mode === "today") {
    title = t("list.todayEmpty.title");
    body = t("list.todayEmpty.body");
  } else {
    // 库真的空了。**这是唯一一屏该给操作指引的地方**——
    // 剪到了东西之后,同样的字再出现在这儿就是废话
    title = t("list.empty.title");
    body = t("list.empty.body");
  }
  const h = document.createElement("p");
  h.className = "empty-title";
  h.textContent = title;
  const b = document.createElement("p");
  b.textContent = body;
  box.append(h, b);

  if (mode === "all" && tag === null) {
    box.append(...buildFirstRunSteps());
  }
  return box;
}

/** 新用户的两步。**都不需要用户离开当前界面**——真正好上手的工具,
 *  不会让新用户先去别处找设置 */
function buildFirstRunSteps(): HTMLElement[] {
  const out: HTMLElement[] = [];
  const steps: Array<[string, string]> = [
    ["1", t("firstRun.paste")],
    ["2", t("firstRun.watch")],
  ];
  for (const [num, text] of steps) {
    const row = document.createElement("div");
    row.className = "first-run-step";
    const dot = document.createElement("span");
    dot.className = "first-run-num";
    dot.textContent = num;
    const label = document.createElement("span");
    label.textContent = text;
    row.append(dot, label);
    out.push(row);
  }

  // **把「开启监控剪贴板」做成一个按钮,而不是一句让人去找的话。**
  // 空库这一屏是用户最愿意点东西的时候,让他自己跑去工具栏找那个
  // 小复选框,是把最简单的场景做成了最麻烦的
  if (!watchOn && !watchAsked) {
    const ask = document.createElement("button");
    ask.className = "btn primary first-run-ask";
    ask.textContent = t("firstRun.askWatch");
    ask.title = t("firstRun.askWatch.title");
    ask.addEventListener("click", () => {
      void askWatchOnce(true);
    });
    out.push(ask);
  }
  return out;
}

/** 列表借给每一条的那几件事。
 *
 *  **做成一个常量,不在每次调用时现写一份。** 三个调用点各写一遍的话,
 *  总有一处会漏掉 `onToggleRead`,而漏掉的那处是在运行时才暴露的——
 *  用户点「标已读」没反应,控制台也不会有任何东西
 */
const clipCtx: ClipItemContext = {
  isActive: (filename) => filename === activeFilename,
  isSelected: (filename) => selected.has(filename),
  onOpen: (event, filename) => onItemClick(event, filename),
  onContextMenu: (event, clip) => openClipContextMenu(event, clip),
  onToggleRead: (clip, next) => void toggleRead(clip, next),
  onToggleStar: (clip, next) => void toggleStar(clip, next),
};

/** 列表右键菜单。
 *
 *  **低频动作全收在这里。** 列表项上已经有一个「已读」快捷按钮和收藏星,
 *  再往卡片上摆归档、删除,用户扫列表时一眼全是按钮;右键是文件管理器
 *  养出来的肌肉记忆,「这一篇还能干什么」的答案就该在右键里。
 *
 *  菜单项跟着这一篇的**当前状态**变:已读的显示「标未读」,收藏过的显示
 *  「取消收藏」——菜单是状态的镜子,不是一排固定按钮。
 *  「删除」排最后并标红:最危险的放最难点到的地方。 */
function openClipContextMenu(event: MouseEvent, clip: ClipSummary): void {
  event.preventDefault();
  const done = clip.read || clip.archived;
  const items: MenuItem[] = [
    {
      label: done ? t("batch.unread") : t("batch.read"),
      title: done ? t("clip.markUnread.title") : t("clip.markRead.title"),
      onClick: () => void toggleRead(clip, !clip.read),
    },
    {
      label: clip.starred ? t("detail.unstar") : t("batch.star"),
      onClick: () => void toggleStar(clip, !clip.starred),
    },
    {
      label: clip.archived ? t("detail.unarchive") : t("detail.archive"),
      onClick: () => void toggleArchive(clip.filename),
    },
  ];
  if (/^https?:\/\//i.test(clip.url)) {
    items.push({
      label: t("detail.openOriginal"),
      onClick: () => void api.openUrl(clip.url),
    });
  }
  items.push({
    label: t("detail.reveal"),
    onClick: () => void openClipFile(clip.filename, true),
  });
  items.push({
    label: t("batch.delete"),
    danger: true,
    onClick: () => void trashClip(clip.filename),
  });
  openContextMenuAt(event.clientX, event.clientY, items);
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

/** 收藏/取消收藏。收藏**不是归档**:归档是读完挪到一边,收藏是一直留着。 */
async function toggleStar(clip: ClipSummary, next: boolean): Promise<void> {
  try {
    const updated = await api.setClipFlags(clip.filename, undefined, undefined, next);
    const i = clips.findIndex((c) => c.filename === clip.filename);
    if (i >= 0) clips[i] = updated;
    if (hits) {
      for (const hit of hits) {
        if (hit.summary.filename === clip.filename) hit.summary = updated;
      }
    }
    // 当前正开着"收藏"这一栏时,取消收藏的那一篇该从列表里消失,
    // 不消失的话用户会看着一个"收藏"视图里没收藏的东西
    renderList();
  } catch (err) {
    showBackendError("error.starFailed", err);
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
    // 指纹跟着一起换。**改完标志这一篇已经不一样了**——留着旧指纹的话,
    // 用户下一次改标题或批注会撞上一个自己制造的冲突
    activeStamp = clip.stamp ?? null;
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
  let report: BatchReport;
  try {
    report = await trashMany([filename]);
  } catch (err) {
    // 整个请求炸了(网络盘掉线、内部锁中毒),不是"某几篇没删成"那种
    // 部分失败。屏幕上一点变化都没有的话,用户分不清是"删不掉"还是
    // "我压根没点到"——批量那条路会弹红条,单篇这条原来一声不吭
    showBackendError("error.trashFailed", err);
    return;
  }
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

/** 就地把详情页的标题换成输入框。判定逻辑在 `rename.ts`,那边有测试盯着。 */
function startRename(clip: ClipContent, heading: HTMLElement): void {
  renameTitle(clip, heading, t("detail.rename"), (filename, title) => {
    void saveTitle(filename, title);
  });
}

async function saveTitle(filename: string, title: string): Promise<void> {
  // 覆盖重试。**这一条必须不带指纹**——带了就是把几秒前那份盖回去,
  // 而那正是守卫要拦的事。冲突只能由用户显式解除
  const write = async (stamp?: string) => {
    const updated = await api.setClipTitle(filename, title, stamp);
    clips = upsertClip(clips, updated);
    // 正文和滚动位置一个字都不重排,只把标题换掉
    const heading = detailEl.querySelector<HTMLElement>(".detail-title");
    if (heading) heading.textContent = updated.title;
    renderList();
  };
  try {
    await write(activeStamp ?? undefined);
    // 写成功了指纹就得换新的。不换的话,下一笔改的还是这一篇时,
    // 会拿**自己刚写的那份**去跟旧指纹比,自己跟自己冲突
    activeStamp = (await api.readClip(filename)).stamp ?? null;
    clearError();
  } catch (err) {
    if (isConflict(err)) reportConflict(() => write(undefined));
    else showBackendError("error.renameFailed", err);
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
  title.className = "detail-title";
  // 剪藏回来的标题是网页给的,用户八成想换成自己认得的样子。
  // 做成点一下就地编辑,而不是往工具栏塞个「重命名」按钮——改标题
  // 是高频动作,不该每次都去别处翻
  title.tabIndex = 0;
  title.setAttribute("role", "button");
  title.title = t("detail.rename.title");
  const beginRename = () => startRename(clip, title);
  title.addEventListener("click", beginRename);
  title.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      beginRename();
    }
  });

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

  const body = document.createElement("article");
  body.className = "prose";
  // 唯一使用 innerHTML 的地方,内容已过 DOMPurify
  body.innerHTML = renderMarkdown(clip.body);
  // 本地化过的图片写的是相对路径,WebView 里直接用就是 404 裂图——
  // 换成本地 asset 地址去读盘上的文件
  resolveAssetImages(body, clipsBase(), convertFileSrc);
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
    // 图片加载会**撑高正文**,可滚空间跟着变——不重算的话,进度条停在
    // 图片加载前算出的旧值上,明明拉到了底,线还差一截。只重算显示,
    // 不写盘:存进度是滚动的事,图片加载不该替用户记一笔
    img.addEventListener("load", recalibrateRestore, { once: true });
  }

  const actions = document.createElement("div");
  actions.className = "detail-actions";
  // 收藏开关。和归档摆在一起,但**不是一回事**——用户在这两件事上的
  // 犹豫方向正好相反:归档是"这篇读完了吧",收藏是"这篇我还想留着"
  const star = document.createElement("button");
  star.className = clip.starred ? "btn ghost on" : "btn ghost";
  star.textContent = clip.starred ? t("detail.starred") : t("detail.star");
  star.title = clip.starred ? t("detail.unstar.title") : t("detail.star.title");
  star.setAttribute("aria-pressed", String(clip.starred));
  star.addEventListener("click", () => void toggleStar(clip, !clip.starred));
  const archive = document.createElement("button");
  archive.className = "btn ghost";
  archiveBtn = archive;
  archive.addEventListener("click", () => void toggleArchive(clip.filename));
  const remove = document.createElement("button");
  remove.className = "btn danger";
  remove.textContent = t("detail.delete");
  remove.title = t("detail.delete.title");
  remove.addEventListener("click", () => void trashClip(clip.filename));
  // 「用默认程序打开」和「在文件夹里定位」。README 写着"数据是你的",
  // 可兑现这句话的只有工具栏那个"打开剪藏目录"——想拿 Obsidian 看某篇,
  // 或者想确认某个附件在不在,都只能自己去文件夹里按文件名找
  const openFile = document.createElement("button");
  openFile.className = "btn ghost";
  openFile.textContent = t("detail.openFile");
  openFile.title = t("detail.openFile.title");
  openFile.addEventListener("click", () => void openClipFile(clip.filename, false));
  const reveal = document.createElement("button");
  reveal.className = "btn ghost";
  reveal.textContent = t("detail.reveal");
  reveal.title = t("detail.reveal.title");
  reveal.addEventListener("click", () => void openClipFile(clip.filename, true));
  actions.append(openFile, reveal, star, archive, remove);
  // 「补全全文」。**只在这一篇看着像"只存了个片段"时才摆**——
  // 每篇都挂一个"重试抓全文"是在给一个用不上的功能找存在感
  const refill = buildRefetchButton(clip, body);
  if (refill) actions.append(refill);
  syncArchiveButton(clip);

  header.append(title, meta, actions);
  // 进度条在正文之前、批注编辑器之后:进度条之下就是正文,一条线
  // 把"我写的"和"我读的"分开
  detailEl.append(header, tagEditor(clip), noteEditor(clip), body);
  detailEl.scrollTop = 0;
  trackReadingProgress(clip.filename, clip.progress);
}

/* ── 批注 ── */

/** 详情页里的批注框。**打错字不要紧,存不上要出声。**
 *
 *  批注是整个 Quire 里唯一一块用户自己从零写的内容——标签和标题都是
 *  认文章,批注是认自己当时的想法。它一旦静默丢一次,用户就不会再往里
 *  写第二次了,所以存盘失败必须摆到界面上,而不是咽下去。
 *
 *  存盘的排队、Esc 撤回、Ctrl+Enter 立刻存,都在 `note.ts` 里,那边有测试。
 */
function noteEditor(clip: ClipContent): HTMLElement {
  const box = document.createElement("section");
  box.className = "note-editor";

  const label = document.createElement("label");
  label.className = "note-label";
  label.textContent = t("detail.note");
  label.htmlFor = "note-text";

  const area = document.createElement("textarea");
  area.id = "note-text";
  area.className = "note-text";
  area.rows = 3;
  area.value = clip.note;
  area.placeholder = t("detail.note.placeholder");
  area.setAttribute("aria-describedby", "note-hint");

  const hint = document.createElement("p");
  hint.id = "note-hint";
  hint.className = "note-hint";
  hint.textContent = t("detail.note.hint");

  attachNoteEditor(area, { filename: clip.filename, note: clip.note }, {
    onSave: async (filename, note) => {
      // 列表行不显示批注,但摘要得跟上:导出、搜索之外的路径
      // (比如标记已读)会拿 clips 里的旧摘要回写,那就白改了
      const write = async (stamp?: string) => {
        const updated = await api.setClipNote(filename, note, stamp);
        clips = upsertClip(clips, updated);
      };
      try {
        await write(activeStamp ?? undefined);
        // 换新指纹,理由同 `saveTitle`:不换的话下一笔会自己跟自己冲突
        activeStamp = (await api.readClip(filename)).stamp ?? null;
      } catch (err) {
        // **只拦冲突这一种。** 其余的照旧抛给 `onError`,那边才有资格去改
        // 提示行的文案;在这儿也写一遍就成了同一个错显示两次
        //
        // 冲突时不碰这个输入框。用户半句话还在里头,重画一下就没了,
        // 而他这半句正是最该留住的东西——只有 `reportConflict` 里那个
        // 「丢掉我的改动」会重画,那是他自己点的
        if (!isConflict(err)) throw err;
        reportConflict(() => write(undefined));
        // 接着抛:得让 `note.ts` 把输入框标成红的。用户得看得见
        // "这一下没存上",不然他还以为已经存了呢
        throw err;
      }
    },
    onError: (err) => {
      // 冲突的提示 `onSave` 里已经报过了,这里别再报一遍
      if (isConflict(err)) return;
      hint.textContent = t("error.noteFailed", { detail: wireText(err) });
      showBackendError("error.noteFailed", err);
    },
    onSaved: () => {
      // 存上了就把提示行换回去。那句"没存上"是过去的事,留着等于
      // 骗用户说刚才那段没存进去。冲突时压根走不到这里(`note.ts` 只在
      // 成功后才调它),所以不用在这儿再分叉
      hint.textContent = t("detail.note.hint");
    },
  });

  box.append(label, area, hint);
  return box;
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
/** 标签栏是不是展开着。
 *
 *  **默认折叠,但能一直展开。** 折叠是为了列表不被挤掉,展开是为了那第十三个
 *  标签不是不存在。以前没有展开这条路,超出的那些只能靠鼠标悬停才看得见,
 *  而没有人会为了看一个标签去悬停——于是它们等于没有
 *
 *  **展开状态只活在内存里,不落盘。** 它是这一次的临时决定,不是偏好。
 *  存进配置的话,用户某次展开了一次,以后每次启动都摊着一屏标签——
 *  而他多半只是想找某一个 */
let tagsExpanded = false;

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
      // 数字比"归档的不算"更说明问题:点了才发现是 0,不如先摆着。
      // 顺带把"右键能改"摆出来——**藏起来的入口等于没有**,
      // 用户想合并标签时不会想到去右键一个标签
      chip.title = on
        ? t("tag.filter.active", { tag: item.tag })
        : `${t("tag.count", { n: item.count })} · ${t("tag.menu")}`;
      chip.addEventListener("click", () => {
        // 再点一下同一个 = 取消筛选。这是最基本的可逆操作,
        // 找不着出口的话用户会以为标签筛进去就出不来了
        tagFilter = on ? null : item.tag;
        // 换了标签就是换了一批东西,**额度得从头数**。不拨的话,
        // 用户在上一个标签下点过两次「显示更多」,切过来第一眼
        // 就是一个已经翻了两屏的列表
        resetListWindow();
        renderTagBar();
        renderList();
      });
      // 右键改名。**不塞进左键菜单里**:左键那个是筛选,用户天天点,
      // 上面挂个"改这个标签"只会让人多点错一下
      chip.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        openTagRenameDialog(item.tag);
      });
      return chip;
    }),
    // **折叠时给一个能展开的按钮,展开完了给一个能收起的按钮。**
    // 旧版这里是一个 `<span>`,名字挂在 `title` 上——一个月后标签有三十个,
    // **等于十八个标签不存在**:用户不会去悬停一个没有暗示的文字,
    // 他只会以为那个标签没打上
    ...(rest > 0
      ? [
          (() => {
            const more = document.createElement("button");
            more.className = "tag-bar-more";
            more.type = "button";
            more.textContent = t("tag.bar.more", { n: rest });
            more.title = t("tag.bar.more.title");
            more.addEventListener("click", () => {
              tagsExpanded = true;
              renderTagBar();
            });
            return more;
          })(),
        ]
      : []),
    ...(tagsExpanded && tags.length > TAG_BAR_LIMIT
      ? [
          (() => {
            const less = document.createElement("button");
            less.className = "tag-bar-more";
            less.type = "button";
            less.textContent = t("tag.bar.less");
            less.title = t("tag.bar.less.title");
            less.addEventListener("click", () => {
              tagsExpanded = false;
              renderTagBar();
            });
            return less;
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
/** 进度条显示的是**滚动位置**,不是"读完了没有"。
 *
 *  两个语义差着一整个产品判断:不足一屏的文章按"完成度"理解就是打开即
 *  100%,可用户拉一下滚轮发现线纹丝不动,那不是"读完了",是"坏了"。
 *  位置语义下,不足一屏 = 没有可滚的空间 = 线留在 0%——和滚轮的手感一致。
 *  写盘跟着这个值走,0 不进文件,短文不会有假进度 */
function readingProgress(): number {
  const scrollable = detailEl.scrollHeight - detailEl.clientHeight;
  if (scrollable <= 8) return 0;
  return Math.min(1, detailEl.scrollTop / scrollable);
}

/** 只重画进度条,不碰存盘。图片加载、窗口缩放这类**布局自己长高**的
 *  时刻用:可滚空间变了,线该跟上;但用户没有滚,进度不该记一笔 */
function syncProgressBarOnly(): void {
  progressBarEl.style.height = `${Math.round(readingProgress() * 100)}%`;
}

/** 挂上滚动监听,停手 1.5 秒后把进度写盘。
 *
 *  打开时先按文件里记的进度跳回去——"读到哪儿了"这个功能有一半的价值
 *  在于**重新打开时接着读**。`requestAnimationFrame` 是为了等布局稳定,
 *  刚 render 完就量 scrollHeight,拿到的可能还是上一篇的旧值。 */
/** 打开一篇多久之后才"算读过"。**翻列表时手快的话一篇也待不了这么久**,
 * 那种"读过"记下来只会让下次启动停在一篇随手划过去的文章上 */
const LAST_READ_AFTER_MS = 10_000;
let lastReadTimer: ReturnType<typeof setTimeout> | null = null;

/** 打开一篇满 10 秒才记成"上次读的那篇" */
function scheduleLastRead(filename: string): void {
  if (lastReadTimer) clearTimeout(lastReadTimer);
  lastReadTimer = setTimeout(() => {
    lastReadTimer = null;
    void api.rememberLastRead(filename).catch(() => {
      /* 记不住不是故障,不打断用户正在读的东西 */
    });
  }, LAST_READ_AFTER_MS);
}

/** 打开时的位置恢复还欠着多少。**用户一滚就作废**——恢复只服务
 *  "接着上次的读",用户自己动了滚动条,他的意图就比记录大 */
let pendingRestore: number | null = null;
/** 恢复滚动是程序自己改 `scrollTop`,会触发 scroll 事件。**不垫这个
 *  标志,恢复的滚动会被当成用户滚动**,把「用户没滚过」的判定冲掉 */
let restoringScroll = false;

function trackReadingProgress(filename: string, saved: number): void {
  detachProgress();
  scheduleLastRead(filename);
  pendingRestore = saved > 0 ? saved : null;
  // 竖线从栏顶往下长:读多少,线多长。方向和滚动同向,一眼能对上
  progressBarEl.style.height = `${Math.round(saved * 100)}%`;
  if (saved > 0) {
    restoreSavedPosition();
  }

  const onScroll = (): void => {
    // 恢复引发的滚动不算用户滚动:不记进度,也不作废待校准的恢复
    if (restoringScroll) {
      restoringScroll = false;
      return;
    }
    pendingRestore = null;
    const value = readingProgress();
    progressBarEl.style.height = `${Math.round(value * 100)}%`;
    // 角标显示的是**阅读位置**,是给眼睛看的——滚动当下就得跟上。
    // 写盘另有 1.5 秒防抖,那管的是磁盘,不该让列表的百分比跟着等
    syncClipProgressBadge(filename, value);
    // 写进队列的是**这一刻**的位置。定时器到点再取,取到的才是最后滚到的那儿
    progressQueue.put({ filename, value });
    if (progressTimer) clearTimeout(progressTimer);
    progressTimer = setTimeout(() => {
      progressTimer = null;
      flushPendingProgress();
    }, PROGRESS_SAVE_DELAY);
  };
  detailEl.addEventListener("scroll", onScroll, { passive: true });
  progressScroll = { filename, onScroll };
}

/** 把滚动位置放回 saved 比例处。**直接用的 scrollHeight 是图片没加载时
 *  的**——懒加载图片一撑高正文,恢复到的位置就漂走(0.56 恢复到 75%
 *  的位置实测过)。所以恢复要做两段:打开瞬间先放个大概,图片加载完
 *  再校准;用户在这期间自己滚了,校准就作废 */
function restoreSavedPosition(): void {
  restoringScroll = true;
  requestAnimationFrame(() => {
    if (pendingRestore == null) return;
    const saved = pendingRestore;
    const scrollable = detailEl.scrollHeight - detailEl.clientHeight;
    if (scrollable > 8) detailEl.scrollTop = scrollable * saved;
    // rAF 里改 scrollTop 触发的 scroll 是异步派发的,标志得等下一帧再撤
    requestAnimationFrame(() => {
      restoringScroll = false;
    });
  });
}

/** 图片加载完把布局撑高了。**用户还没滚过的话,按最新的可滚空间把
 *  位置重新校准一遍**;滚过了就什么都不做,用户的意图比记录大 */
function recalibrateRestore(): void {
  if (pendingRestore != null) restoreSavedPosition();
  syncProgressBarOnly();
}

/** 摘掉进度监听和待写的定时器。**每个重画详情的地方都得调**:
 *  监听器挂在 `detailEl` 上而不是正文节点上,换一篇不会自动摘,
 *  读第二篇时第一篇的滚动也会往它自己的文件里写进度。 */
function detachProgress(): void {
  // **先把手上的这一次写出去,再摘定时器。** 原来是直接 clearTimeout:
  // 用户滚到一半(定时器还差 1.2 秒)点开下一篇,这一篇的进度就永远停在
  // 原地了——而进度条还挂在他眼前,显示着刚才那个位置。
  // 丢的是用户的位置,存的是一次写盘,不值得为了省这一次去换
  flushPendingProgress();
  if (progressScroll) {
    detailEl.removeEventListener("scroll", progressScroll.onScroll);
    progressScroll = null;
  }
  // 线归零必须跟着监听一起摘。**摘了监听不归零的话,回收站详情这类
  // 没有滚动监听的页面会继承上一篇的线**,拉到顶也不动——看起来就是
  // 进度条坏了。renderDetail 这条路紧接着 trackReadingProgress 会
  // 重新赋值,这里归零不会造成闪烁
  progressBarEl.style.height = "0%";
  // 待校准的恢复也作废:上一篇的恢复重放到这一篇上就是乱跳
  pendingRestore = null;
}

/** 把还挂在定时器上的那次进度立刻写出去。**没有待写的就什么都不做**——
 *  定时器已经触发过、或者一次都没滚过,都不该白写一次盘。 */
function flushPendingProgress(): void {
  if (progressTimer) {
    clearTimeout(progressTimer);
    progressTimer = null;
  }
  const pending = progressQueue.take();
  if (pending) void saveProgress(pending.filename, pending.value);
}

/** 把列表右上角的百分比同步成这一刻的阅读位置。
 *
 *  **列表只在重画时读 `clips`,而滚动写盘不触发重画**——不同步的话,
 *  百分比冻在扫描那一刻的值上:用户明明拉回了开头,列表还挂着 100%,
 *  怎么等都不动。这里两份都要跟上:内存里的 `clips` 是下一次重画的
 *  来源,`.clip-half` 是此刻屏幕上挂着的那个角标。
 *
 *  滚动事件一秒几十次,而角标只在**跨过整数百分点**时才真的变——
 *  同一格就整个短路,连内存都不改,不然两千篇的库每次滚动都要全表
 *  find 加 querySelector */
function syncClipProgressBadge(filename: string, progress: number): void {
  const clip = clips.find((c) => c.filename === filename);
  const shown = Math.round(progress * 100);
  if (clip) {
    if (Math.round(clip.progress * 100) === shown) return;
    clip.progress = progress;
  }

  const item = listEl.querySelector<HTMLElement>(
    `.clip[data-filename="${CSS.escape(filename)}"]`,
  );
  if (!item) return;
  const half = item.querySelector<HTMLElement>(".clip-half");
  if (progress > 0 && clip && !clip.read && !clip.archived) {
    // 和 clip-item 的渲染条件一致:读了一半才挂角标。渲染时挂在末尾
    // (读屏先念标题),这里补建也挂末尾,两边别长成两副样子
    if (!half) {
      const badge = document.createElement("span");
      badge.className = "clip-half";
      item.append(badge);
      badge.textContent = `${shown}%`;
    } else {
      half.textContent = `${shown}%`;
    }
  } else {
    half?.remove();
  }
}


async function saveProgress(filename: string, progress: number): Promise<void> {
  // 已经读到 100% 的,以后再打开也不该被"读了一半"的进度条盖住。
  // 后端只认"值变了才写",但界面这一侧也得先想清楚
  if (progress >= 0.999) progress = 1;
  try {
    await api.setClipProgress(filename, progress);
    syncClipProgressBadge(filename, progress);
    const outcome = progressJudge.judge(true);
    if (outcome === "recovered") showToast(t("toast.progressRecovered"));
  } catch (err) {
    // 原来这里是空 catch,理由写的是"记不住进度是小事,不该打断"。
    // 理由站不住:进度条还在屏幕上显示着他读到哪儿,用户读完关掉,
    // 下次打开回到零,他不会想到是写盘失败,只会觉得 Quire 记不住读到哪儿了。
    // **报一次,不是每次都报**——滚动停 1.5 秒就写一次,磁盘抖一下能连着
    // 失败十几次,每次弹一条比不提示还糟(用户会开始无视所有提示)
    if (progressJudge.judge(false) === "warn") {
      showToast(t("toast.progressFailed", { detail: wireText(err) }));
    }
  }
}

function renderEmptyDetail(): void {
  detachProgress();
  detailEl.replaceChildren();
  // 进度条是栏边的全局元素,不随详情重建。空态时归零,别留着上一篇的长度
  progressBarEl.style.height = "0%";
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
    // 记下这是**哪一份**。往后在这篇上改任何东西,后端都会拿它比一次
    activeStamp = clip.stamp ?? null;
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
    detailEl.replaceChildren(
      fallback,
      note,
      trashActions(filename, null, trash.find((t) => t.filename === filename)?.quarantined === true),
    );
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
  const item = trash.find((t) => t.filename === clip.filename);
  header.append(title, meta, trashActions(clip.filename, item?.sizeBytes ?? null, item?.quarantined === true));

  const body = document.createElement("article");
  body.className = "prose";
  // 唯一使用 innerHTML 的地方,内容已过 DOMPurify
  body.innerHTML = renderMarkdown(clip.body);
  resolveAssetImages(body, clipsBase(), convertFileSrc);
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
function trashActions(
  filename: string,
  sizeBytes: number | null,
  quarantined: boolean,
): HTMLElement {
  const actions = document.createElement("div");
  actions.className = "detail-actions";

  const back = document.createElement("button");
  back.className = "btn primary";
  back.textContent = t("trash.restore");
  back.title = t("trash.restore.title");
  back.addEventListener("click", () => void restoreFromTrash(filename));

  // **两个按钮后果完全不同,文案也必须不同。** 回收站里那个"彻底删除"
  // 只是搬去隔离区,还能翻回来;隔离区里那个才是真删。摆成一个样子的
  // danger 按钮,用户迟早会照着隔离区里那个的力度去点回收站里那个
  const purge = document.createElement("button");
  purge.className = "btn danger";
  if (quarantined) {
    purge.textContent = t("trash.forget");
    purge.title = sizeBytes === null
      ? t("trash.forget.title")
      : t("trash.forget.titleWithSize", { size: formatBytes(sizeBytes) });
    purge.addEventListener("click", () => void forgetOne(filename));
  } else {
    purge.textContent = t("trash.purge");
    purge.title = sizeBytes === null
      ? t("trash.purge.title", { days: quarantineDays })
      : t("trash.purge.titleWithSize", {
          size: formatBytes(sizeBytes),
          days: quarantineDays,
        });
    purge.addEventListener("click", () => void purgeFromTrash(filename));
  }

  actions.append(back, purge);
  return actions;
}

/** 真删一篇。**这是全软件唯一不可撤销的操作**,所以不能只靠按钮上那句
 *  "没有撤销"——用户是闭着眼点的。走一次系统确认框,是他睁眼签字的最后机会 */
async function forgetOne(filename: string): Promise<void> {
  if (!window.confirm(t("confirm.forget"))) return;
  // 位置要在**删之前**记下来。删完再查 `visibleFilenames()`,这一篇已经不在里面了,
  // indexOf 永远返回 -1,选中就会跳到列表最前面——用户以为列表被清空了
  const at = visibleFilenames().indexOf(filename);
  try {
    await api.forgetClip(filename);
    trash = trash.filter((i) => i.filename !== filename);
    if (activeFilename === filename) {
      activeFilename = null;
      renderTrashList();
      // 删掉一篇之后选中它原来那个位置的下一篇,而不是跳回第一篇
      const next = reanchorAfterRemoval(visibleFilenames(), at);
      if (next) void openTrashDetail(next);
      else renderEmptyDetail();
    } else {
      renderTrashList();
    }
    showToast(t("toast.forgotten"));
  } catch (err) {
    showError(wireText(err));
  }
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

/** 把一篇从回收站搬到隔离区。**搬完它还在界面上,只是标成了"已隔离"**——
 *  这不是界面偷懒,是承诺:用户点完这一下,回头还能在同一个列表里翻回来。
 *  从列表上抹掉等于骗他说"没了",他真想要回来时连找都找不着 */
async function purgeFromTrash(filename: string): Promise<void> {
  try {
    await api.purgeClip(filename);
    trash = trash.map((t) =>
      t.filename === filename ? { ...t, quarantined: true } : t,
    );
    renderTrashList();
    // 详情页要重画:两个按钮会从"移到隔离区"变成"彻底删除",不重画就还是旧文案
    if (activeFilename === filename) void openTrashDetail(filename);
    showToast(t("toast.quarantined", { days: quarantineDays }));
  } catch (err) {
    showBackendError("error.purgeFailed", err);
  }
}

/** 隔离区保留期。**只信后端下发的**,不从 i18n 里抠文案反推——那是循环依赖。
 *  拉列表失败时先落一个占位值,真拉到就覆盖掉;界面上会因此短暂显示
 *  「? 天后自动清理」,好过显示一个自己编的 30 */
let quarantineDays = 0;

async function refreshTrash(): Promise<void> {
  const listing = await api.listTrash();
  trash = listing.items;
  quarantineDays = listing.quarantineDays;
}

/** 重拉标签栏。**拉不到就清空**:留着上一次的数据等于告诉用户
 *  「这些标签现在还是这样」,而实际上已经变了——宁可空着让他重新点。
 *
 *  但**清空的同时必须出声**。标签栏是用户组织剪藏的主要抓手,它突然整条
 *  消失又不给一句话,用户只会认定"标签丢了",然后去逐个 `.md` 确认——
 *  而实际上只是 IPC 抖了一下。同一个文件里 `applyTags` 失败是弹红条的,
 *  这里原来一声不吭,两处同类失败一个弹一个静默,用户会认为软件不稳定。 */
async function refreshTags(): Promise<void> {
  if (filter === "trash") return;
  try {
    tags = await api.listTags();
  } catch (err) {
    tags = [];
    showBackendError("error.tagsUnavailable", err);
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
  // **先看看能不能抓到更全的。** 用户从网页上复制一小段的时候,
  // 那一小段之外的文章他多半也想要——过去这条路只能靠扩展
  const enriched = await tryFetchFullText(capture);
  const { markdown, title, excerpt, meta } = clipToMarkdown(enriched);
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

/** 尽力把全文捞回来。
 *
 *  ## 什么时候值得试
 *
 *  **复制的内容短,才值得去抓。** 用户已经复制了几千字的正文,
 *  那就是全文,再抓一遍是白花一次网络往返,还多等好几秒。
 *  阈值定得偏保守:500 字大概是一篇普通博客文章的长度
 *
 *  ## 抓不到怎么办
 *
 *  **原样存,不多说一句话。** 需要登录的页面、纯 JS 渲染的、被拦的,
 *  都是常态。用户复制的那段字照样存得下来,这才是他真正要的东西。
 *  弹一条红条只会让他以为剪藏失败了
 *
 *  抓到的 HTML 里可能带着标题、作者、发布时间——那些一并补上,
 *  扩展那条路能给的元数据,这条路大部分也能给
 */
async function tryFetchFullText(capture: ClipboardCapture): Promise<ClipboardCapture> {
  const url = capture.url?.trim();
  if (!url || !/^https?:\/\//i.test(url)) return capture;
  // 已经有扩展给的元数据了,说明这条路已经拿到全文,不必再抓
  if (capture.meta && Object.keys(capture.meta).length > 0) return capture;
  // 复制的内容已经很长,那就是全文
  if (capture.text.trim().length > FULL_TEXT_MIN_CLIP_CHARS) return capture;

  try {
    const page = await api.fetchArticle(url);
    if (!page.html.trim()) return capture;
    // **包成和扩展一样的形状**,走 clipToMarkdown 那条已经测过的路。
    // 另开一条抽取路径等于给同一件事写两份实现,迟早不一致
    return {
      text: capture.text,
      html: page.html,
      url: page.url || url,
      meta: capture.meta ?? {},
    };
  } catch {
    // 抓不到是正常结局,不是故障。**静默**:用户要的是存下来,
    // 至于多存了还是少存了全文,他复制的那段字本来就是对的
    return capture;
  }
}

/** 复制的内容短于这个字数才值得去抓全文。500 字大概是普通博客文章的长度 */
const FULL_TEXT_MIN_CLIP_CHARS = 500;

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

/** 搜索范围。**空搜索框时它不起作用**,但仍然保留——用户换个词就还用得上,
 * 每次清空都重置一遍只会让人多按几下 */
let searchScope: SearchScope = "any";

async function runSearch(query: string, remember = true): Promise<void> {
  const trimmed = query.trim();
  // 换了个词就是换了一批结果,额度从头数。**不拨的话上限会串台**:
  // 用户在上一次搜索里点过两次「显示更多」,这一次搜出来的东西
  // 第一眼就是翻过两屏的样子,而他这次还没翻过
  resetListWindow();
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
    const result = inTrash
      ? await api.searchTrash(trimmed, undefined, searchScope)
      : await api.searchClips(trimmed, undefined, searchScope);
    // 用户可能已经改词、换视图或清空了。这次的返回值过期,丢掉
    if (searchEl.value.trim() !== trimmed || filter === "trash" !== inTrash) return;
    hits = result.hits;
    // **只在真搜出东西的时候才记。** 搜了个错的词、或者范围限得太窄,
    // 把它记进"你刚才搜过"里,用户下一次点进去又搜不到——那一栏本来就该
    // 帮他想起搜过什么,不是垃圾桶。点历史里的词重搜时传 false,
    // 免得刚点完就把列表重排了
    if (remember && result.hits.length > 0) {
      void api.rememberSearch(trimmed).then(refreshRecentSearches, () => {});
    }
    // **跳过的那些必须说出来。** 列表视图那边已经用红条告诉过用户
    // "有 N 篇读不出",一搜那几篇就没了还不吭声——他明明记得那篇里有
    // 这个词,搜出来是空的,只会以为是自己记错了
    if (result.skipped.length > 0) {
      showError(
        t("error.searchSkipped", {
          n: result.skipped.length,
          names: result.skipped.map((f) => f.filename).join("、"),
        }),
      );
    } else {
      clearError();
    }
    renderList();
  } catch (err) {
    showBackendError("error.searchFailed", err);
  }
}

/** 最近搜过的词。**只认后端给的那一份,前端不自己攒。**
 *
 * 两份数据迟早对不上:用户在前端这份里去掉一条、后端那份还在,
 * 下一次启动那一条又冒出来,而他已经不记得搜过它了
 */
let recentSearches: string[] = [];

async function refreshRecentSearches(): Promise<void> {
  try {
    recentSearches = await api.recentSearches();
  } catch {
    // 读不出来就当没有。**这一栏本来就是锦上添花**——它在,用户少打一次字;
    // 它不在,用户照样能搜。为它弹一条红条,是拿红杠换一个可有可无的便利
    recentSearches = [];
  }
}

// 每敲一下就全库扫一遍,剪藏多了会跟着手抖。去抖 200ms 是体感不明显的下限。
// 每个选项的悬停说明。**下拉框选中之后看不见自己选的是什么**,
// 一排「全部 / 标题 / 正文 / 标签 / 批注」摆在那儿,用户只能靠猜
//
// **键写成字面量而不是拼出来的**:拼出来的键,i18n 的完整性测试扫不到,
// 少一条文案不会有人发现,界面上就是一个没有说明的下拉框
const SCOPE_HINT: Record<SearchScope, string> = {
  any: "search.scope.any.hint",
  title: "search.scope.title.hint",
  body: "search.scope.body.hint",
  tag: "search.scope.tag.hint",
  note: "search.scope.note.hint",
};
const scopeEl = el<HTMLSelectElement>("search-scope");
for (const opt of scopeEl.options) {
  const hint = t(SCOPE_HINT[opt.value as SearchScope]);
  opt.title = hint;
  opt.setAttribute("aria-label", hint);
}

// 换范围要立刻重搜。**不重搜的话界面会停在上一个范围的结果上**,
// 而搜索框里的词没变——用户会以为这个范围不起作用
scopeEl.addEventListener("change", (e) => {
  searchScope = (e.target as HTMLSelectElement).value as SearchScope;
  void runSearch(searchEl.value);
});

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

/**
 * 手动刷新。**全仓库只有这一条刷新路径**——按钮、`F5`、窗口聚焦重扫
 * 三个入口都走它。
 *
 * 另写一套的话,过一阵两条路就会分叉:一条清了红条另一条没清、
 * 一条刷了回收站另一条没刷。这类分叉很难查,因为两条路**看起来都对**。
 *
 * **正在搜的时候不切回全库。** 用户输入 "rust"、结果 8 条,随手按一下
 * `F5`——列表忽然变成几百条,而搜索框里还写着 "rust",
 * 他会以为搜索坏了。判定在 `refresh.ts`。
 */
async function refreshNow(): Promise<void> {
  if (afterRefresh(searchEl.value) === "reload") {
    hits = null;
  }
  await refreshList();
}

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
    vaultRoot = info.path;
    vaultPathEl.textContent = info.path;
    vaultPathEl.title = info.path;
    watchOn = info.watching;
    watchAsked = info.watchAsked;
    // 剪藏库建不出来(只读盘、网盘掉线)时 `list_clips` 会返回一个空列表,
    // 界面上就是个干干净净的空库。**必须把真实原因摆出来**,否则用户
    // 的第一反应是"我的剪藏全没了",然后去翻硬盘、翻回收站、翻云同步
    if (info.problem) {
      // **按代号分。** "上次的目录不见了"和"目录打不开"是两回事,
      // 一律套 `vaultUnavailable` 的话,用户看到的是"打不开剪藏库:剪藏目录不存在",
      // 而他真正该知道的是"你的剪藏多半还在,把它接回去"
      // `wireText` 自己会按代号取对应那句、并把 `{path}` 填上,不用在这儿拼
      if (isWireError(info.problem) && info.problem.code === "vault.missing") {
        showError(wireText(info.problem));
      } else {
        showBackendError("error.vaultUnavailable", info.problem);
      }
    }
  } catch {
    vaultPathEl.textContent = t("error.vaultUnknown");
  }
}

el<HTMLButtonElement>("btn-paste").addEventListener("click", () => void pasteNow());

async function openClipFile(filename: string, reveal: boolean): Promise<void> {
  try {
    await api.openClipFile(filename, reveal);
  } catch (e) {
    showError(wireText(e));
  }
}

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
  // 换了视图就是换了一批东西,额度从头数。理由同标签那处
  resetListWindow();
  filter = next;
  for (const [button, value] of [
    [filterAllEl, "all"],
    [filterTodayEl, "today"],
    [filterUnreadEl, "unread"],
    [filterWeekEl, "week"],
    [filterStarredEl, "starred"],
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
filterTodayEl.addEventListener("click", () => void setFilter("today"));
filterUnreadEl.addEventListener("click", () => void setFilter("unread"));
filterWeekEl.addEventListener("click", () => void setFilter("week"));
filterStarredEl.addEventListener("click", () => void setFilter("starred"));
filterArchivedEl.addEventListener("click", () => void setFilter("archived"));
filterTrashEl.addEventListener("click", () => void setFilter("trash"));

/** 列表头部那个按钮的逻辑。**回收站和隔离区是两个地方,后果差着"能不能撤销"**:
 *  - 回收站 → 隔离区:还能翻回来
 *  - 隔离区 → 真删:不可撤销,而且这才真正腾得出磁盘
 *
 * 所以隔离区为空时**不该摆"清空隔离区"的按钮**出来——摆一个点下去什么都不干的
 * 按钮,比不摆更让人怀疑是不是程序坏了。主按钮一律写成"取消" */
function askEmptyTrash(): void {
  const inTrash = trash.filter((i) => !i.quarantined).length;
  const inQuarantine = trash.length - inTrash;
  if (inTrash === 0 && inQuarantine === 0) return;
  if (inQuarantine === 0) {
    showToast(t("confirm.emptyTrash", { n: inTrash, days: quarantineDays }), [
      { label: t("batch.clear"), primary: true, onClick: hideToast },
      { label: t("confirm.emptyTrashAction"), onClick: () => void doEmptyTrash() },
    ]);
    return;
  }
  showToast(t("confirm.emptyBoth", { trash: inTrash, quarantine: inQuarantine }), [
    { label: t("batch.clear"), primary: true, onClick: hideToast },
    { label: t("confirm.quarantineAll"), onClick: () => void doEmptyTrash() },
    { label: t("confirm.forgetAll"), onClick: () => void doForgetAll() },
  ]);
}

/** 把隔离区里那些**真的删掉**。这是全软件唯一不可撤销的地方,所以确认两次才动手:
 * 上面的 toast 已经问过一次"你要删几条",这里再来一个系统确认框——
 * 单次点击 + 一排并排的红色按钮,太容易顺着就点了 */
async function doForgetAll(): Promise<void> {
  hideToast();
  const n = trash.filter((i) => i.quarantined).length;
  if (!window.confirm(t("confirm.forgetAllBody", { n }))) return;
  try {
    const report = await api.forgetAll();
    const failed = new Set(report.failed.map((f) => f.filename));
    // 清不掉的留在列表上,不能一股脑清空本地状态
    trash = trash.filter((t) => failed.has(t.filename));
    activeFilename = null;
    renderTrashList();
    renderEmptyDetail();
    if (report.failed.length > 0) {
      showError(
        t("error.forgetAllPartial", {
          ok: report.succeeded.length,
          n: report.failed.length,
          names: report.failed.map((f) => f.filename).join("、"),
        }),
      );
    } else {
      showToast(t("toast.forgottenAll", { n: report.succeeded.length }));
    }
  } catch (err) {
    showBackendError("error.forgetAllFailed", err);
  }
}

async function doEmptyTrash(): Promise<void> {
  hideToast();
  try {
    const report = await api.emptyTrash();
    // 失败的还在回收站里,得留在列表上,不能一股脑清空本地状态
    const failed = new Set(report.failed.map((t) => t.filename));
    // **搬去隔离区的那些要留在列表上并标成已隔离**——它们没被删掉,
    // 从界面上抹掉等于骗用户说"没了",回头想翻回来就找不着了
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
      showToast(
        t("toast.clearedTrash", {
          n: report.succeeded.length,
          days: quarantineDays,
        }),
      );
    }
  } catch (err) {
    showBackendError("error.emptyTrashFailed", err);
  }
}

/* ── 剪藏历史:刚才弹出来过、还没存的 ── */

/** 抽屉一次最多摆多少行。**内存里留 200 条,但摆 200 行的话
 *  用户要滚很久才看完,而他真正想找的多半是最近那几条 */
const HISTORY_ROW_LIMIT = 60;

let historyEl: HTMLElement | null = null;

/* ── 状态:库里有多少东西没做完 ── */

const statusEl = el<HTMLButtonElement>("btn-status");
let statusPanelEl: HTMLElement | null = null;
/** 上一次查出来的状态。**别重复查**——`library_status` 要读一遍全库,
 *  而这个面板的唯一用途就是用户点开来瞄一眼 */
let lastStatus: LibraryStatus | null = null;

function statusIssueCount(s: LibraryStatus | null): number {
  if (!s) return 0;
  return s.remoteImages.length + s.missingFulltext.length + s.unreadable.length;
}

/** 只有真有欠账才摆那个按钮。
 *
 *  一个永远在那儿、点进去写着「一切都好」的按钮是给人添乱的——
 *  用户会为了确认它是不是坏了而多点它两下 */
function updateStatusBadge(): void {
  const n = statusIssueCount(lastStatus);
  if (n === 0) {
    statusEl.hidden = true;
    statusEl.textContent = "";
    return;
  }
  statusEl.hidden = false;
  statusEl.textContent = t("status.badge", { n });
  const full = t("status.badge.title", { n });
  statusEl.title = full;
  statusEl.setAttribute("aria-label", full);
}

async function refreshStatus(): Promise<void> {
  try {
    lastStatus = await api.libraryStatus();
  } catch {
    // **读不出来就当没有欠账。** 报「状态未知」会让用户以为出了什么大事,
    // 而这一栏本来就是锦上添花
    lastStatus = null;
  }
  updateStatusBadge();
  if (statusPanelEl) renderStatusPanel();
}

/** 状态面板。**每一项都要写清「这是什么、怎么办」。**
 *
 * 只列问题不给动作的面板等于白做——用户看到「12 篇的图还指着外站」,
 * 不知道该拿它怎么办,那他就只会关掉,然后继续不知道 */
function openStatus(): void {
  closeStatus();
  const box = document.createElement("aside");
  box.className = "history-drawer status-panel";
  box.id = "status-panel";
  box.setAttribute("role", "dialog");
  box.setAttribute("aria-label", t("status.title"));
  statusPanelEl = box;
  renderStatusPanel(box);
  document.body.append(box);
  document.addEventListener("keydown", onStatusKey);
}

function renderStatusPanel(into?: HTMLElement): void {
  const box = into ?? statusPanelEl;
  if (!box) return;
  box.replaceChildren();

  const head = document.createElement("div");
  head.className = "history-head";
  const title = document.createElement("h2");
  title.textContent = t("status.title");
  const close = document.createElement("button");
  close.className = "btn ghost";
  close.textContent = t("status.close");
  close.addEventListener("click", () => closeStatus());
  head.append(title, close);

  const list = document.createElement("div");
  list.className = "status-list";
  const s = lastStatus;

  if (!s || statusIssueCount(s) === 0) {
    const ok = document.createElement("div");
    ok.className = "empty";
    const p = document.createElement("p");
    p.className = "empty-title";
    p.textContent = t("status.allGood");
    ok.append(p);
    list.append(ok);
  } else {
    if (s.remoteImages.length > 0) {
      list.append(
        statusSection(
          "status.remoteImages",
          s.remoteImages.map((c) => ({
            label: c.title,
            detail: t("status.remoteImages.item", { n: c.count }),
            filename: c.filename,
          })),
        ),
      );
    }
    if (s.missingFulltext.length > 0) {
      list.append(
        statusSection(
          "status.missingFulltext",
          s.missingFulltext.map((c) => ({
            label: c.title,
            detail: t("status.missingFulltext.item"),
            filename: c.filename,
          })),
        ),
      );
    }
    if (s.unreadable.length > 0) {
      list.append(
        statusSection(
          "status.unreadable",
          s.unreadable.map((c) => ({
            label: c.filename,
            detail: c.reason,
            filename: c.filename,
          })),
        ),
      );
    }
  }

  box.append(head, list);
}

/** 一屏最多摆几行。**摆二十行用户不会看**,而且每一行都得点得中,
 *  摆在第一屏之外的等于没摆 */
const STATUS_ROW_LIMIT = 8;

function statusSection(
  headingKey: string,
  items: Array<{ label: string; detail: string; filename: string }>,
): HTMLElement {
  const sec = document.createElement("section");
  sec.className = "status-section";
  const h = document.createElement("h3");
  h.textContent = t(headingKey);
  sec.append(h);
  for (const item of items.slice(0, STATUS_ROW_LIMIT)) {
    const row = document.createElement("button");
    row.className = "status-row";
    const label = document.createElement("span");
    label.className = "status-row-label";
    label.textContent = item.label;
    const detail = document.createElement("span");
    detail.className = "status-row-detail";
    detail.textContent = item.detail;
    row.append(label, detail);
    // **点一行直接打开那篇。** 只报问题不给动作,用户看完还是不知道怎么办
    row.addEventListener("click", () => {
      void openDetail(item.filename);
      closeStatus();
    });
    sec.append(row);
  }
  if (items.length > STATUS_ROW_LIMIT) {
    const more = document.createElement("p");
    more.className = "history-more";
    more.textContent = t("status.more", { n: items.length - STATUS_ROW_LIMIT });
    sec.append(more);
  }
  return sec;
}

function onStatusKey(e: KeyboardEvent): void {
  if (e.key === "Escape") closeStatus();
}

function closeStatus(): void {
  statusPanelEl?.remove();
  statusPanelEl = null;
  document.removeEventListener("keydown", onStatusKey);
}

statusEl.addEventListener("click", () => {
  if (statusPanelEl) {
    closeStatus();
  } else {
    openStatus();
  }
});

/* ── 快捷键面板 ── */

const helpEl = el<HTMLButtonElement>("btn-help");
let shortcutPanelEl: HTMLElement | null = null;

/** 快捷键全表。
 *
 * **清单在 `shortcuts.ts` 里,不在这个函数里手写。** 手写的话,加一个新键
 * 十有八九忘了回来补——而面板上少一条的后果是那个键**等于不存在**:
 * 用户不知道自己能这么用,也就一直不会用,功能白做
 */
function renderShortcutPanel(into: HTMLElement): void {
  into.replaceChildren();

  const head = document.createElement("div");
  head.className = "history-head";
  const title = document.createElement("h2");
  title.textContent = t("shortcut.title");
  const close = document.createElement("button");
  close.className = "btn ghost";
  close.textContent = t("shortcut.close");
  close.addEventListener("click", () => closeShortcuts());
  head.append(title, close);
  into.append(head);

  for (const group of SHORTCUTS) {
    const section = document.createElement("div");
    section.className = "shortcut-group";
    const heading = document.createElement("h3");
    heading.className = "shortcut-group-title";
    heading.textContent = t(group.title);
    section.append(heading);

    for (const item of group.items) {
      const row = document.createElement("div");
      row.className = "shortcut-row";
      const keys = document.createElement("span");
      keys.className = "shortcut-keys";
      // 修饰键在前、主键在后,而且**每段各是一个 `<kbd>`**——
      // 全塞进一个 kbd 里,"Ctrl" 和 "A" 之间就没有那道缝,
      // 看着像一个叫 CtrlA 的键
      for (const cap of item.caps) {
        const kbd = document.createElement("kbd");
        kbd.textContent = cap;
        keys.append(kbd);
      }
      const label = document.createElement("span");
      label.className = "shortcut-label";
      label.textContent = t(item.label);
      row.append(keys, label);
      section.append(row);
    }
    into.append(section);
  }
}

function openShortcuts(): void {
  closeShortcuts();
  const box = document.createElement("aside");
  box.className = "history-drawer shortcut-panel";
  box.id = "shortcut-panel";
  box.setAttribute("role", "dialog");
  box.setAttribute("aria-label", t("shortcut.title"));
  shortcutPanelEl = box;
  renderShortcutPanel(box);
  document.body.append(box);
  helpEl.setAttribute("aria-expanded", "true");
}

function closeShortcuts(): void {
  shortcutPanelEl?.remove();
  shortcutPanelEl = null;
  helpEl.setAttribute("aria-expanded", "false");
}

helpEl.addEventListener("click", () => {
  if (shortcutPanelEl) closeShortcuts();
  else openShortcuts();
});

/* ── 这库里有什么 ── */

const statsEl = el<HTMLButtonElement>("btn-stats");
let statsPanelEl: HTMLElement | null = null;

/** 站点、标签那两栏最多列几条。
 *
 *  **列不完的要说出来还剩多少。** 少一句「还有 12 个」的话,
 *  用户会以为他这辈子只在八个站点上存过东西 */
const STATS_RANK_LIMIT = 8;

/** 一根柱子的最低高度(百分比)。
 *
 *  **为零的那格不画,这是有意的。** 给它也留一条底线的话,
 *  "这个月一篇没存"和"这个月存了两篇"在图上长得一模一样——
 *  而前者恰恰是这一页最该让人看见的事 */
const BAR_MIN = 4;

function statsSection(titleKey: string): HTMLElement {
  const box = document.createElement("div");
  box.className = "stats-block";
  const h = document.createElement("h3");
  h.className = "stats-block-title";
  h.textContent = t(titleKey);
  box.append(h);
  return box;
}

/** 排行:一条一个名字一条横杠。`onPick` 给了就能点(用来筛)。 */
function statsRank(
  rows: Array<{ name: string; count: number }>,
  more: number,
  moreKey: string,
  onPick?: (name: string) => void,
): HTMLElement {
  const box = document.createElement("div");
  box.className = "stats-rank";
  const max = Math.max(1, ...rows.map((r) => r.count));
  for (const row of rows) {
    const line = document.createElement(onPick ? "button" : "div");
    line.className = "stats-rank-row";
    if (onPick) line.setAttribute("type", "button");

    const name = document.createElement("span");
    name.className = "stats-rank-name";
    // 空站点是"手工放进去的那些",给一句人话,别显示一个空名字
    name.textContent = row.name === "" ? t("stats.noSite") : row.name;
    // 一条横杠比一个数字好扫:**长短一眼看得出**,而 37 和 41
    // 要停下来比一比。数字还是留着,精确的答案它才给得了
    const bar = document.createElement("span");
    bar.className = "stats-rank-bar";
    const fill = document.createElement("span");
    fill.className = "stats-rank-fill";
    fill.style.width = `${Math.max(BAR_MIN, (row.count / max) * 100)}%`;
    bar.append(fill);

    const n = document.createElement("span");
    n.className = "stats-rank-n";
    n.textContent = String(row.count);

    line.append(name, bar, n);
    if (onPick) {
      line.title = t("stats.pickTag", { tag: row.name, n: row.count });
      line.addEventListener("click", () => onPick(row.name));
    }
    box.append(line);
  }
  if (more > 0) {
    const rest = document.createElement("p");
    rest.className = "stats-more";
    rest.textContent = t(moreKey, { n: more });
    box.append(rest);
  }
  return box;
}

/** 「这库里有什么」。
 *
 *  **算的是内存里那份 `clips`,不重扫磁盘。** 面板一打开就等一次全库
 *  扫描的话,用户点它之前得先想"值不值得等";而这一页的意义就在于
 *  随手看一眼
 */
function renderStatsPanel(into: HTMLElement): void {
  into.replaceChildren();
  const s = libraryStats(clips);

  const head = document.createElement("div");
  head.className = "history-head";
  const title = document.createElement("h2");
  title.textContent = t("stats.title");
  const close = document.createElement("button");
  close.className = "btn ghost";
  close.textContent = t("stats.close");
  close.addEventListener("click", () => closeStats());
  head.append(title, close);
  into.append(head);

  if (s.total === 0) {
    const none = document.createElement("div");
    none.className = "empty";
    const p = document.createElement("p");
    p.className = "empty-title";
    p.textContent = t("stats.empty");
    none.append(p);
    into.append(none);
    return;
  }

  // 概览。**先给几个大头**——用户目光停在这儿的时候最多
  const grid = document.createElement("div");
  grid.className = "stats-grid";
  for (const [key, value] of [
    ["stats.total", s.total],
    ["stats.unread", s.unread],
    ["stats.halfRead", s.halfRead],
    ["stats.withNote", s.withNote],
  ] as const) {
    const cell = document.createElement("div");
    cell.className = "stats-cell";
    const num = document.createElement("span");
    num.className = "stats-num";
    num.textContent = String(value);
    const label = document.createElement("span");
    label.className = "stats-label";
    label.textContent = t(key);
    cell.append(num, label);
    grid.append(cell);
  }
  into.append(grid);

  // 按月。时间从左到右,旧的在左
  const months = statsSection("stats.byMonth");
  const chart = document.createElement("div");
  chart.className = "stats-chart";
  const bars = document.createElement("div");
  bars.className = "stats-bars";
  const max = Math.max(1, ...s.byMonth.map((m) => m.count));
  for (const m of s.byMonth) {
    const bar = document.createElement("div");
    bar.className = "stats-bar";
    const fill = document.createElement("div");
    fill.className = "stats-bar-fill";
    // 零就是零。**别给它留底线**——见 `BAR_MIN` 的注释
    fill.style.height = m.count === 0 ? "0" : `${Math.max(BAR_MIN, (m.count / max) * 100)}%`;
    bar.append(fill);
    bar.title = t("stats.monthBar", { month: m.month, n: m.count });
    bars.append(bar);
  }
  chart.append(bars);
  const axis = document.createElement("div");
  axis.className = "stats-axis";
  const first = document.createElement("span");
  const [fy, fm] = s.byMonth[0].month.split("-");
  first.textContent = t("stats.monthLabel", { year: fy, month: Number(fm) });
  const last = document.createElement("span");
  const [ly, lm] = s.byMonth[s.byMonth.length - 1].month.split("-");
  last.textContent = t("stats.monthLabel", { year: ly, month: Number(lm) });
  axis.append(first, last);
  chart.append(axis);
  months.append(chart);
  into.append(months);

  // 站点:看的是"我平时都在哪儿存东西"。sites 那边字段叫 `site`,
  // tags 那边叫 `tag`,这里统一成 `name`——排行那个函数两边都要用,
  // 为它俩各写一份渲染不值得
  const sites = statsSection("stats.bySite");
  sites.append(
    statsRank(
      s.bySite.slice(0, STATS_RANK_LIMIT).map((x) => ({ name: x.site, count: x.count })),
      s.bySite.length - STATS_RANK_LIMIT,
      "stats.moreSites",
    ),
  );
  into.append(sites);

  // 标签:**点一下就按它筛**,这是这一页唯一一个"看完能接着干点什么"
  // 的地方——统计的意义不是看数字,是看完知道下一步干什么
  const tags = statsSection("stats.byTag");
  if (s.byTag.length === 0) {
    const none = document.createElement("p");
    none.className = "stats-more";
    none.textContent = t("stats.noTags");
    tags.append(none);
  } else {
    tags.append(
      statsRank(
        s.byTag.slice(0, STATS_RANK_LIMIT).map((x) => ({ name: x.tag, count: x.count })),
        s.byTag.length - STATS_RANK_LIMIT,
        "stats.moreTags",
        (tag) => {
          tagFilter = tag;
          resetListWindow();
          closeStats();
          renderTagBar();
          renderList();
        },
      ),
    );
  }
  into.append(tags);
}

function openStats(): void {
  closeStats();
  const box = document.createElement("aside");
  box.className = "history-drawer stats-panel";
  box.id = "stats-panel";
  box.setAttribute("role", "dialog");
  box.setAttribute("aria-label", t("stats.title"));
  statsPanelEl = box;
  renderStatsPanel(box);
  document.body.append(box);
  statsEl.setAttribute("aria-expanded", "true");
}

function closeStats(): void {
  statsPanelEl?.remove();
  statsPanelEl = null;
  statsEl.setAttribute("aria-expanded", "false");
}

statsEl.addEventListener("click", () => {
  if (statsPanelEl) closeStats();
  else openStats();
});

/* ── 设置 ── */

/** 一行设置:左边是名字和后果,右边是控件。
 *
 *  说明写的是「关掉会怎样」而不是「这个开关是干什么的」——名字已经说了一遍,
 *  用户真正要判断的是**动了它会怎样**,重复一遍名字只会让人更快地划过去 */
function settingsRow(
  titleKey: string,
  noteKey: string,
  control: HTMLElement,
): HTMLElement {
  const row = document.createElement("div");
  row.className = "settings-row";
  const text = document.createElement("div");
  text.className = "settings-text";
  const title = document.createElement("h3");
  title.className = "settings-row-title";
  title.textContent = t(titleKey);
  const note = document.createElement("p");
  note.className = "settings-note";
  note.textContent = t(noteKey);
  text.append(title, note);

  const slot = document.createElement("div");
  slot.className = "settings-control";
  slot.append(control);
  row.append(text, slot);
  return row;
}

function settingsSection(titleKey: string): HTMLElement {
  const box = document.createElement("section");
  box.className = "settings-section";
  const h = document.createElement("h2");
  h.className = "settings-section-title";
  h.textContent = t(titleKey);
  box.append(h);
  return box;
}

/** 画设置面板。**每次状态变了就整个重画**,不做局部更新——
 *  这里一共七八个元素,重画的代价是零,而局部更新意味着「改库路径忘了改
 *  旁边那句说明」这类只有手动同步才躲得过的错 */
function renderSettingsPanel(box: HTMLElement): void {
  const title = document.createElement("h2");
  title.className = "settings-title";
  title.id = "settings-title";
  title.textContent = t("settings.title");

  const body = document.createElement("div");
  body.className = "settings-body";

  const vault = settingsSection("settings.vault");
  const path = document.createElement("code");
  path.className = "settings-path";
  path.textContent = vaultRoot || t("settings.path.empty");
  path.title = vaultRoot;

  const vaultBtns = document.createElement("div");
  vaultBtns.className = "settings-buttons";
  const openBtn = document.createElement("button");
  openBtn.className = "btn";
  openBtn.textContent = t("toolbar.open");
  openBtn.addEventListener("click", () => {
    void api.openVaultFolder().catch((e) => showError(wireText(e)));
  });
  const changeBtn = document.createElement("button");
  changeBtn.className = "btn";
  changeBtn.textContent = t("settings.changeDir");
  changeBtn.addEventListener("click", () => void changeVaultWithMigration());
  vaultBtns.append(openBtn, changeBtn);
  vault.append(path, vaultBtns);

  const clip = settingsSection("settings.clipboard");
  const watchBox = document.createElement("label");
  watchBox.className = "switch";
  const watchInput = document.createElement("input");
  watchInput.type = "checkbox";
  watchInput.checked = watchOn;
  watchInput.addEventListener("change", () => {
    void setWatchToggle(watchInput.checked);
  });
  const watchText = document.createElement("span");
  watchText.textContent = t("settings.watch");
  watchBox.append(watchInput, watchText);
  clip.append(
    settingsRow("settings.watch", "settings.watch.note", watchBox),
  );

  const lang = settingsSection("settings.lang");
  const langBtn = document.createElement("button");
  langBtn.className = "btn";
  langBtn.textContent = localeName();
  langBtn.title = t("lang.switch");
  langBtn.addEventListener("click", () => {
    setLocale(otherLocale());
    applyLocale();
  });
  lang.append(langBtn);

  body.append(vault, clip, lang);
  box.replaceChildren(title, body);
}

function openSettings(): void {
  closeSettings();
  const box = document.createElement("aside");
  box.className = "history-drawer settings-panel";
  box.id = "settings-panel";
  box.setAttribute("role", "dialog");
  box.setAttribute("aria-label", t("settings.title"));
  settingsPanelEl = box;
  renderSettingsPanel(box);
  document.body.append(box);
  settingsBtnEl.setAttribute("aria-expanded", "true");
}

function closeSettings(): void {
  settingsPanelEl?.remove();
  settingsPanelEl = null;
  settingsBtnEl.setAttribute("aria-expanded", "false");
}

/** 面板开着的时候把它重画一遍。**没开着就什么都不做**——
 *  空面板被顺手清掉的话,用户刚点开设置它就消失了 */
function syncSettingsPanel(): void {
  if (!settingsPanelEl) return;
  renderSettingsPanel(settingsPanelEl);
}

settingsBtnEl.addEventListener("click", () => {
  if (settingsPanelEl) closeSettings();
  else openSettings();
});


/** 打开 / 切换。**挂在 body 上而不是列表里**——它是跨视图的:
 *  用户在详情页、在回收站里,都可能想翻回刚才那批 */
function openHistory(): HTMLElement | null {
  if (historyEl) {
    closeHistory();
    return null;
  }
  const box = document.createElement("aside");
  box.className = "history-drawer";
  box.id = "history-drawer";
  box.setAttribute("role", "dialog");
  box.setAttribute("aria-label", t("history.title"));

  const head = document.createElement("div");
  head.className = "history-head";
  const title = document.createElement("h2");
  title.textContent = t("history.title");
  const close = document.createElement("button");
  close.className = "btn ghost";
  close.textContent = t("history.close");
  close.addEventListener("click", () => closeHistory());
  head.append(title, close);

  const list = document.createElement("div");
  list.className = "history-list";

  const pending = clipHistory.pending();
  if (pending.length === 0) {
    const empty = document.createElement("div");
    empty.className = "empty";
    const why = document.createElement("p");
    why.className = "empty-title";
    why.textContent = t("history.empty");
    empty.append(why);
    list.append(empty);
  } else {
    for (const item of pending.slice(0, HISTORY_ROW_LIMIT)) {
      list.append(historyRow(item));
    }
    // 超出上限的**明说**。内存里只留 200 条,超出那部分是真的没了,
    // 悄悄少摆几行最让人困惑——他会以为"就这些了"
    if (pending.length > HISTORY_ROW_LIMIT) {
      const more = document.createElement("p");
      more.className = "history-more";
      more.textContent = t("history.more", {
        n: pending.length - HISTORY_ROW_LIMIT,
      });
      list.append(more);
    }
    const clearAll = document.createElement("button");
    clearAll.className = "btn ghost history-clear";
    clearAll.textContent = t("history.clearAll");
    clearAll.addEventListener("click", () => {
      clipHistory.clear();
      renderHistoryList();
      updateHistoryBadge();
    });
    list.append(clearAll);
  }

  box.append(head, list);
  document.body.append(box);
  historyEl = box;
  // 焦点挪进抽屉。**开着抽屉焦点还在外面**,键盘用户 Tab 一圈都回不来,
  // 读屏软件也还在念上一个地方
  close.focus();
  updateHistoryBadge();
  // 返回它自己,而不是让调用方去读模块级变量:那条路会碰上控制流分析
  // 把变量窄化成 never,改一处代码、另一处类型就跟着塌
  return box;
}

function historyRow(item: clipHistory.HistoryItem): HTMLElement {
  const row = document.createElement("article");
  row.className = "history-item";

  const preview = document.createElement("p");
  preview.className = "history-text";
  const first =
    item.text.trim().split("\n").find((l) => l.trim()) ?? "";
  preview.textContent = first.slice(0, 120) || t("toast.detectedFallback");

  const meta = document.createElement("p");
  meta.className = "history-meta";
  meta.textContent =
    t("history.meta", { when: formatWhen(new Date(item.lastSeen).toISOString()) }) +
    (item.seen > 1 ? t("history.seen", { n: item.seen }) : "");

  const save = document.createElement("button");
  save.className = "btn primary";
  save.textContent = t("toast.save");
  save.addEventListener("click", () => void restoreFromHistory(item));

  row.append(preview, meta, save);
  return row;
}

/** 从历史里补存一篇。**走和当场存完全一样的那条路**——
 *  另开一条路就会出现"补存的没下图片""补存的没判重"这类只在某个
 *  入口下才有的问题,而那种 bug 最难被测出来 */
async function restoreFromHistory(item: clipHistory.HistoryItem): Promise<void> {
  const capture: ClipboardCapture = {
    text: item.text,
    html: item.html,
    url: item.url,
    meta: item.meta,
  };
  try {
    const duplicate = await saveCapture(capture);
    clipHistory.markSaved(item.id);
    closeHistory();
    if (duplicate) announceDuplicate(duplicate, capture);
    else showToast(t("toast.saved"));
    await refreshList();
  } catch (err) {
    showError(wireText(err));
  }
}

/** 抽屉开着的时候原地重画。**存完一条之后不能整个关掉**——
 *  用户往往是连着补存好几条,每存一条就重开一次抽屉,那比不开还累 */
function renderHistoryList(): void {
  const box: HTMLElement | null = historyEl;
  if (!box) return;
  const list = box.querySelector<HTMLElement>(".history-list");
  const scroll = list?.scrollTop ?? 0;
  box.remove();
  historyEl = null;
  const fresh = openHistory();
  // 重画之后把滚动位置放回去。用户是连着补存好几条的,
  // 每存一条就跳回顶部的话,第二条得重新往下翻才看得见
  const next = fresh?.querySelector<HTMLElement>(".history-list");
  if (next) next.scrollTop = scroll;
}

function closeHistory(): void {
  historyEl?.remove();
  historyEl = null;
  updateHistoryBadge();
}

/** 入口按钮。**没东西的时候整个隐掉**——一个常驻的「历史(0)」
 *  只会让人多看一眼然后忘掉,而真需要找的时候他自然会看见它出现 */
function updateHistoryBadge(): void {
  const btn = document.getElementById("btn-history");
  if (!btn) return;
  const n = clipHistory.pending().length;
  btn.hidden = n === 0;
  btn.textContent = t("history.button", { n });
  btn.title = t("history.button.title");
  btn.setAttribute("aria-label", t("history.title"));
}

/* ── 批量标签 ── */

/** 批量标签面板。列的是**库里已有的标签**,不是让用户从零敲——
 *  打标签本来就是把同一类归到一起,让人重新拼一遍已有标签名
 *  是把「整理」变成「录入」。 */
function renderBatchTagPanel(): void {
  batchTagPanelEl.hidden = !batchTagPanelOpen;
  if (!batchTagPanelOpen) return;

  const picked = new Set(batchTagsPicked);
  const chips = document.createElement("div");
  chips.className = "tag-chips";
  if (tags.length === 0) {
    const none = document.createElement("p");
    none.className = "tag-bar-empty";
    none.textContent = t("tag.bar.empty");
    chips.append(none);
  } else {
    chips.append(
      ...tags.map((item) => {
        const on = picked.has(item.tag);
        const chip = document.createElement("button");
        chip.type = "button";
        chip.className = on ? "tag-chip filter active" : "tag-chip filter";
        chip.setAttribute("aria-pressed", String(on));
        chip.textContent = item.tag;
        chip.title = t("tag.count", { n: item.count });
        chip.addEventListener("click", () => {
          batchTagsPicked = on
            ? batchTagsPicked.filter((x) => x !== item.tag)
            : [...batchTagsPicked, item.tag];
          renderBatchTagPanel();
        });
        return chip;
      }),
    );
  }

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
    e.preventDefault();
    const next = withTag(batchTagsPicked, input.value);
    // 返回原数组说明没加上(重复或空),清掉输入框免得用户以为自己加上了
    if (next === batchTagsPicked) {
      input.value = "";
      return;
    }
    batchTagsPicked = next;
    input.value = "";
    renderBatchTagPanel();
    input.focus();
  });

  const actions = document.createElement("div");
  actions.className = "batch-tag-actions";
  const ok = document.createElement("button");
  ok.className = "btn primary";
  ok.type = "button";
  ok.textContent = t("batch.tagsApply");
  ok.addEventListener("click", () => void applyBatchTags());
  const cancel = document.createElement("button");
  cancel.className = "btn ghost";
  cancel.type = "button";
  cancel.textContent = t("batch.clear");
  cancel.addEventListener("click", closeBatchTagPanel);
  actions.append(ok, cancel);

  const note = document.createElement("p");
  note.className = "tag-hint";
  note.textContent = t("batch.tagsReplace");

  batchTagPanelEl.replaceChildren(chips, form, actions, note);
}

function closeBatchTagPanel(): void {
  batchTagPanelOpen = false;
  batchTagsPicked = [];
  renderBatchTagPanel();
}

/** 一次给选中的全打上这几个标签。**后端是替换不是追加**,所以选区里
 *  原来带的标签会被换掉——面板上明写了这一点。 */
async function applyBatchTags(): Promise<void> {
  const names = selectedFiles();
  if (names.length === 0) {
    closeBatchTagPanel();
    return;
  }
  // **动手之前先拍。** 标签是"替换"不是"追加",不拍就没法撤销——
  // 而拍晚了中间那一步改动就漏掉了
  const snaps = snapshotClips(clips, names, { tags: true });
  try {
    const report = await api.setClipTagsBatch(names, batchTagsPicked);
    const done = new Set(report.succeeded);
    // 成功的从选区里去掉,失败的留着——用户看得见"哪几篇没成",可以再点一次
    selected = new Set([...selected].filter((f) => done.has(f)));
    // 批量报告只带回文件名,不带改完的摘要。与其在内存里把标签拼回去
    // (拼错了就是"界面显示已打标签、文件里其实没有"),不如老实重扫一遍磁盘
    await refreshList();
    if (activeFilename) await refreshOpenDetail();
    closeBatchTagPanel();
    // **改成功了才给撤销。** 没改成功的那些旧值原封不动,
    // 给个撤销按钮点了必然是一次空写
    offerBatchUndo(
      restorable(snaps, report),
      report,
      t("batch.tagsDone", { n: report.succeeded.length }),
    );
  } catch (err) {
    showBackendError("error.batchTagsFailed", err);
  }
}

/** 批量改动之后,把撤销摆在旁边。
 *
 * **只有真改成功的才有得撤销。** 这不是抠门:给没改成的篇一个撤销,
 * 用户点下去会看到"已撤销",而实际上什么都没变——他会以为这条路验过了,
 * 于是下次更敢批量操作
 *
 * 跟批量删除的撤销是同一个道理:有退路之后,确认框不再是最后一道闸,
 * 它本来就只是让人别手滑
 */
function offerBatchUndo(
  snapshots: ClipSnapshot[],
  report: BatchReport,
  done: string,
): void {
  if (report.failed.length > 0) {
    showError(
      t("error.batchPartial", {
        done,
        n: report.failed.length,
        reason: wireText(report.failed[0].reason),
      }),
    );
  }
  if (snapshots.length === 0) {
    // 一篇都没改成:没有可撤销的东西,也别报"改了几篇"
    if (report.failed.length === 0) showToast(done);
    return;
  }
  showToast(t("toast.batchDone", { done, n: snapshots.length }), [
    {
      label: t("toast.undo"),
      primary: true,
      onClick: () => void runBatchUndo(snapshots),
    },
    { label: t("toast.close"), onClick: hideToast },
  ]);
}

/** 撤销一次批量改动。
 *
 * **逐篇发,不合成批量。** 批量命令只能"全设成同一个值",而快照里每篇
 * 的旧值不一样——把它们全设成同一串,撤销一次反而造出一批新错。
 * 慢一点是值的:一个批量 20 篇,多 20 次 IPC 也就几十毫秒,
 * 而"撤销之后比撤销之前更乱"是用户再也不信这个功能的开始 */
async function runBatchUndo(snapshots: ClipSnapshot[]): Promise<void> {
  hideToast();
  let out: BatchUndo;
  try {
    out = await undoBatch(snapshots, async (group, kind) => {
      // **逐篇发,并且如实记账。** 一律回一句"全成功"的话,
      // 中间哪一篇真写失败了就被吞了——用户看着"已撤销",回头一看
      // 那篇还带着原来的标签,而他完全不知道
      const succeeded: string[] = [];
      const failed: BatchReport["failed"] = [];
      for (const snap of group) {
        try {
          if (kind === "tags") await api.setClipTags(snap.filename, snap.tags ?? []);
          else {
            await api.setClipFlagsBatch(
              [snap.filename],
              snap.read,
              snap.archived,
              snap.starred,
            );
          }
          succeeded.push(snap.filename);
        } catch (err) {
          failed.push({ filename: snap.filename, reason: toWireError(err) });
        }
      }
      return { succeeded, failed };
    });
  } catch (err) {
    showBackendError("error.undoFailed", err);
    return;
  }
  await refreshList();
  if (activeFilename) await refreshOpenDetail();
  if (out.failed.length === 0) {
    showToast(t("toast.undone", { n: out.restored }));
    return;
  }
  // 退回来了大部分:只说成功的话,没退回来的那几篇用户会以为也退了
  showError(
    t("error.undoPartial", {
      n: out.restored,
      missed: out.failed.length,
      reason: wireText(out.failed[0].reason),
    }),
  );
}

el<HTMLButtonElement>("batch-tags").addEventListener("click", () => {
  batchTagPanelOpen = !batchTagPanelOpen;
  if (!batchTagPanelOpen) batchTagsPicked = [];
  renderBatchTagPanel();
});

/* ── 改标签 ── */

/** 改名对话框目前同时只会有一个,存起来是为了 Esc 能关掉它 */
let tagRenameDialog: HTMLElement | null = null;

/**
 * 问用户把一个标签换成什么。
 *
 * **不用 `window.prompt`**:那是个原生弹窗,样式和字体不受控,
 * 还会挡住整个窗口——用户中途想去看一眼剪藏库都做不成。
 * 也**不做成一个长期的侧边面板**:改标签是一次性动作,做完就消失,
 * 长期占地方是在为"一年用一次"的功能付租金
 */
function openTagRenameDialog(from: string): void {
  closeTagRenameDialog();

  const box = document.createElement("div");
  box.className = "rename-dialog";
  box.setAttribute("role", "dialog");
  box.setAttribute("aria-modal", "true");
  box.setAttribute("aria-label", t("tag.rename.title"));

  const title = document.createElement("h2");
  title.textContent = t("tag.rename.title");
  const bodyText = document.createElement("p");
  bodyText.textContent = t("tag.rename.body", { from });
  const hint = document.createElement("p");
  hint.className = "hint";
  hint.textContent = t("tag.rename.hint");

  const input = document.createElement("input");
  input.type = "text";
  input.className = "rename-input";
  input.value = from;
  input.placeholder = t("tag.rename.input");
  // 选中原名而不是全选:改名前用户多半想接着加字(「待读」→「待读-长文」),
  // 全选一敲就把原名冲掉了
  input.setAttribute("aria-label", t("tag.rename.input"));

  const actions = document.createElement("div");
  actions.className = "rename-actions";
  const cancel = document.createElement("button");
  cancel.className = "btn";
  cancel.textContent = t("toast.close");
  cancel.addEventListener("click", () => closeTagRenameDialog());
  const ok = document.createElement("button");
  ok.className = "btn primary";
  ok.textContent = t("tag.rename.action");
  ok.addEventListener("click", () => {
    const to = input.value.trim();
    // 和原名一样就别提交了,提交了也是白扫一遍磁盘
    if (to && to !== from) void doRenameTag(from, to);
    closeTagRenameDialog();
  });
  actions.replaceChildren(cancel, ok);

  // 回车提交、Esc 取消。**别的键一律不吞**,输入框里要能打中文
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      ok.click();
    } else if (e.key === "Escape") {
      e.preventDefault();
      closeTagRenameDialog();
    }
  });
  // 点遮罩也关掉。点对话框内部不能关——选文字的时候很容易点到边界
  box.addEventListener("click", (e) => {
    if (e.target === box) closeTagRenameDialog();
  });

  box.replaceChildren(title, bodyText, input, hint, actions);
  document.body.appendChild(box);
  tagRenameDialog = box;
  input.focus();
  input.setSelectionRange(0, from.length);
}

function closeTagRenameDialog(): void {
  tagRenameDialog?.remove();
  tagRenameDialog = null;
}

async function doRenameTag(from: string, to: string): Promise<void> {
  try {
    const report = await api.renameTag(from, to);
    // 失败的那几篇要点名。**只报「改了几篇」的话**,用户会以为全改完了,
    // 回头发现有几篇挂着旧标签,只会觉得软件有问题
    if (report.failed.length > 0) {
      showError(
        t("error.renameTagPartial", {
          n: report.changed,
          total: report.failed.length,
          detail: wireText(report.failed[0].reason),
        }),
      );
    }
    if (report.changed === 0 && report.failed.length === 0) {
      showToast(t("tag.rename.nothing", { tag: from }));
      return;
    }
    // 正在按旧标签筛选的话,改完那个筛选就不存在了,得撤掉,
    // 否则列表会空着,而用户不知道该点什么取消
    if (tagFilter === from) tagFilter = null;
    showToast(
      tags.some((i) => i.tag === to)
        ? t("tag.rename.merged", { to })
        : t("tag.rename.renamed", { to }),
    );
    await refreshNow();
  } catch (err) {
    showError(wireText(err));
  }
}

/* ── 批量操作 ── */

function selectedFiles(): string[] {
  return [...selected];
}

/** 一次改多篇的标志。**逐条走 `set_flags` 的同一条路**,所以"不许弄坏
 * 用户文件"那两条规矩在批量下照样成立。 */
async function batchFlags(
  read: boolean | undefined,
  archived: boolean | undefined,
  what: string,
  starred?: boolean,
): Promise<void> {
  const names = selectedFiles();
  if (names.length === 0) return;
  // 动手之前先拍,理由同批量打标签
  const snaps = snapshotClips(clips, names, { read, archived, starred });
  try {
    const report = await api.setClipFlagsBatch(names, read, archived, starred);
    const done = new Set(report.succeeded);
    // 成功的那些从选区里去掉,失败的留着——用户看得见"哪几篇没成",
    // 可以再点一次重试,而不是操作完还得回头猜是哪几篇
    selected = new Set([...selected].filter((f) => done.has(f)));
    // 批量报告只带回文件名,不带改完的摘要。与其在内存里把标志拼回去
    // (拼错了就是"界面说已读、文件里还是未读"),不如老实重扫一遍磁盘
    await refreshList();
    if (activeFilename) await refreshOpenDetail();
    offerBatchUndo(
      restorable(snaps, report),
      report,
      t("batch.flagsDone", { action: what, n: report.succeeded.length }),
    );
  } catch (err) {
    showBackendError("error.batchFailed", err, { action: what });
  }
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
el<HTMLButtonElement>("batch-star").addEventListener("click", () =>
  void batchFlags(undefined, undefined, t("batch.star"), true),
);
el<HTMLButtonElement>("batch-clear").addEventListener("click", () => {
  selected = new Set();
  selectionAnchor = null;
  closeBatchTagPanel();
  renderList();
});

el<HTMLButtonElement>("btn-history").addEventListener("click", () => openHistory());
el<HTMLButtonElement>("batch-export").addEventListener("click", () => void exportSelected());

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
          .then((report) => reportTrash(report))
          .catch((err) => showError(wireText(err)));
      },
    },
  ]);
});

/** 删完之后报结果,**并且把撤销摆在旁边**。
 *
 * 单篇删除有撤销、批量没有,这本身就是不自洽:一次删 20 篇,唯一的保险
 * 变成"下次少勾两篇"。有撤销的时候确认框也就不再是最后一道闸——它本来
 * 就只是让人别手滑按错,不是让人确认"这二十篇我真不要了"。
 */
function reportTrash(report: BatchReport): void {
  const undo = undoableFiles(report);
  // 红条和 toast 是两处,不打架,一起显示:红条说"哪几篇没删成、为什么",
  // toast 说"删了多少、能退回来"。**不因为有失败就把撤销一起收走**——
  // 18 篇删成了就是删成了,用户照样该有退路,收走等于回到批量没撤销的老问题,
  // 只是换了个触发条件
  if (report.failed.length > 0) {
    showError(
      t("error.trashPartial", {
        n: report.failed.length,
        reason: wireText(report.failed[0].reason),
      }),
    );
  }
  // 一篇都没删成,就没有可撤销的东西,也不用说"已移到回收站 0 篇"
  if (undo.length === 0) {
    if (report.failed.length === 0) hideToast();
    return;
  }
  showToast(t("toast.movedMany", { n: undo.length }), [
    { label: t("toast.undo"), primary: true, onClick: () => void undoTrashMany(undo) },
    { label: t("toast.close"), onClick: hideToast },
  ]);
}

/** 撤销一次批量删除。放回来的走 upsert,不是顶到最前面——
 *  放回来的是 20 篇旧剪藏,顶到顶上会把用户整个列表顺序搅乱。 */
async function undoTrashMany(filenames: string[]): Promise<void> {
  hideToast();
  let out: UndeleteResult;
  try {
    out = await undeleteApi(filenames, api.restoreClips);
  } catch (err) {
    showBackendError("error.undoFailed", err);
    return;
  }
  for (const clip of out.restored) clips = upsertClip(clips, clip);
  renderList();
  if (out.failed.length === 0) {
    showToast(t("toast.restoredMany", { n: out.restored.length }));
    return;
  }
  // 退回来了大部分但有几篇没回来:只报成功的话,那几篇用户会以为回来了
  showError(
    t("error.undoPartial", {
      n: out.restored.length,
      missed: out.failed.length,
      reason: wireText(out.failed[0].reason),
    }),
  );
}

el<HTMLButtonElement>("btn-import").addEventListener("click", () => {
  // 工具栏已经有八个按钮了,不再加第九个。**两条导入路径摆成一次选择**:
  // 它们都是"往库里搬东西",拆成两个按钮只会让人猜哪个是哪条路
  showToast(t("import.choose"), [
    { label: t("import.fromFolder"), primary: true, onClick: () => void runFolderImport() },
    { label: t("import.fromFile"), onClick: () => void runFileImport() },
    { label: t("batch.clear"), onClick: hideToast },
  ]);
});

async function runFolderImport(): Promise<void> {
  hideToast();
  try {
    const { report, unreadableDirs } = await api.importMarkdown();
    await refreshList();
    reportImport(report, unreadableDirs);
  } catch (err) {
    // 用户在目录选择器上点了取消,那不是故障。**按代号判,不按中文句子判**:
    // 拿 `String(err).includes("已取消")` 判,文案一改就失效,
    // 而且英文界面下那句中文压根不会出现,取消会被当成真错误弹红字
    if (isWireError(err) && err.code === "app.cancelled") return;
    showBackendError("error.importFailed", err);
  }
}

/** 从 Pocket / 别的工具的 CSV、JSON 导。**三类结果分开说**——
 *  导进来的、库里已有跳过的、读不出来的。合成一句"导入完成 N 篇"的话,
 *  用户分不清那 N 篇是不是他以为的那些,而重复那部分恰恰是他最该知道的 */
async function runFileImport(): Promise<void> {
  hideToast();
  try {
    const report = await api.importDataFile();
    // 用户在文件对话框上点了取消
    if (report === null) return;
    await refreshList();
    reportDataImport(report);
  } catch (err) {
    if (isWireError(err) && err.code === "app.cancelled") return;
    showBackendError("error.importDataFailed", err);
  }
}

function reportDataImport(report: DataImportReport): void {
  const n = report.imported.length;
  const dup = report.duplicates.length;
  const bad = report.failed.length;
  if (n === 0 && dup === 0 && bad === 0) {
    showError(t("error.importNoneEmpty"));
    return;
  }
  if (bad > 0) {
    // 读不出来的**带行号**报出来:用户回他的表格里查那一行,
    // 只报"有一行坏了"他根本不知道是哪一行
    showError(
      t("error.importDataPartial", {
        ok: n,
        n: bad,
        names: report.failed
          .slice(0, 3)
          .map((f) => t("import.row", { row: f.row }))
          .join("、") + (bad > 3 ? t("import.more", { n: bad - 3 }) : ""),
      }),
    );
    return;
  }
  if (dup > 0 && n === 0) {
    // 全是重复。**这不是失败**,但也不是"导入完成"——用户得知道
    // 他导的这些库里都已经有了,不然他会以为功能没生效
    showError(
      t("error.importAllDuplicate", {
        n: dup,
        name: report.duplicates.slice(0, 3).join("、"),
      }),
    );
    return;
  }
  // 有重复也有新进来的,提示里得把重复说出来,不然用户以为全导进去了
  showToast(
    dup > 0
      ? t("toast.importedWithDupes", { n, dup })
      : t("toast.imported", { n }),
  );
}

/** 导入结果。**跳过的那些必须列出来**:用户导进一个存过一堆旧文的文件夹,
 * 里面有一半是重复的,界面只报一句"导入完成"的话,他没法判断到底进来了多少。 */
function reportImport(report: BatchReport, unreadableDirs: string[] = []): void {
  const n = report.succeeded.length;
  // 读不出的目录是**最该先说**的:那不是某几篇失败,是整批文件没进来。
  // 静默过去的话,用户看到"导入完成 240 篇",以为全导完了
  const missed =
    unreadableDirs.length > 0
      ? t("error.importDirsUnreadable", {
          n: unreadableDirs.length,
          names: unreadableDirs.slice(0, 3).join("、") + (unreadableDirs.length > 3 ? ` 等 ${unreadableDirs.length} 个` : ""),
        })
      : "";
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
    // 一篇没失败、但有目录没读进去,不能弹"全部搞定"那种提示。
    // 另外 `toast.imported` 是**好消息**的文案,塞进红条等于把一句好消息
    // 说成了坏消息——红条一亮,用户就认定出事了
    if (missed) showError(missed);
    else showToast(t("toast.imported", { n }));
    return;
  }
  showError(
    [missed, t("error.importPartial", {
      ok: n,
      n: report.failed.length,
      names:
        report.failed
          .slice(0, 3)
          .map((f) => `${f.filename}(${wireText(f.reason)})`)
          .join("、") + (report.failed.length > 3 ? ` 等 ${report.failed.length} 个` : ""),
    })]
      .filter(Boolean)
      .join(" "),
  );
}

el<HTMLButtonElement>("btn-export").addEventListener("click", () => {
  // 导出有两条路,而**默认那条必须是带图的**。剪藏时图片已经存到本地
  // assets/ 里了,只导出一个 .md 的话,用户把它发给别人、在手机上打开,
  // 一张图都裂——"我的数据是我的"这句话在带图剪藏上最该成立,偏偏这条路上失效
  showToast(t("export.choose"), [
    { label: t("export.folder"), primary: true, onClick: () => void runExportFolder() },
    { label: t("export.textOnly"), onClick: () => void runTextExport() },
    { label: t("batch.clear"), onClick: hideToast },
  ]);
});

/** 导出成文件夹。**图片数要报出来**:用户拷到别的机器上之后才发现少了图,
 * 那时候他已经没法回头查了 */
async function runExportFolder(filenames?: string[]): Promise<void> {
  hideToast();
  try {
    const report = await api.exportFolder(filenames);
    if (!report) return; // 用户点了取消,不是故障
    // 有图和没图是两回事。只导了文字还弹一句"图也带上了",是在骗他
    showToast(
      report.images > 0
        ? t("toast.exportedFolder", { n: report.images })
        : t("toast.exportedFolderNoImages"),
    );
  } catch (err) {
    showBackendError("error.exportFailed", err);
  }
}

/** 只要文字的单个 .md。用户导出到笔记软件里时常常就是要这个 */
async function runTextExport(): Promise<void> {
  hideToast();
  try {
    // 返回 null 是用户在保存对话框点了取消,那不是故障,别弹红字
    const path = await api.exportVault();
    if (path) showToast(t("toast.exported"));
  } catch (err) {
    showBackendError("error.exportFailed", err);
  }
}

/** 导出选中的那几篇,带图。走批量栏上的按钮,不占工具栏——工具栏已经八个了,
 *  而这个动作**只在有选中时才有意义**,摆在那儿常年是灰的更让人疑惑。
 *
 *  挑中三篇发给别人却导出了全库两百篇,用户得回去手工删干净的——那还不如
 *  压根没有这个功能。所以它单独一条路,不给"没选中就导全部"这种兜底 */
async function exportSelected(): Promise<void> {
  const filenames = [...selected];
  if (filenames.length === 0) return;
  await runExportFolder(filenames);
}

/** 更改数据目录。选完先问一句「现有的迁不迁」,答了才动库——
 *  不问就切走,等于替用户决定扔不扔他几千篇剪藏 */
async function changeVaultWithMigration(): Promise<void> {
  let target: string | null;
  try {
    target = await api.chooseVaultTarget();
  } catch (err) {
    showBackendError("error.pickVaultFailed", err);
    return;
  }
  if (!target) return; // 取消就是什么都没发生

  // 三选一摆在 toast 上:迁移过去 / 空目录开始 / 取消。**主按钮给「取消」**
  // ——回车和误触落到的都是它,想要迁移得明确点一下那个次要的按钮
  hideToast();
  showToast(t("settings.migrate.body", { n: String(clips.length) }), [
    { label: t("settings.migrate.cancel"), primary: true, onClick: hideToast },
    {
      label: t("settings.migrate.copy"),
      onClick: () => void finishMigration(target, true),
    },
    {
      label: t("settings.migrate.empty"),
      onClick: () => void finishMigration(target, false),
    },
  ]);
}

/** 真正切库。**只有这一处改 `config.json` 里的库路径**,面板上看到的路径
 *  从这里回来,别在别处再改一遍——两处都改,迟早有一处漏了同步 */
async function finishMigration(target: string, copyExisting: boolean): Promise<void> {
  hideToast();
  try {
    await api.migrateVault(target, copyExisting);
    await loadVaultInfo();
    await refreshList();
    syncSettingsPanel();
    // 搬过去的篇数由 `vault-migrated` 事件报。**这里再报一遍就成了两条提示
    // 互相顶掉**,用户看见的是后到的那条,篇数直接没了
    if (!copyExisting) showToast(t("settings.migrate.switched"));
  } catch (err) {
    showBackendError("error.vault.migrateFailed", err);
  }
}

/** 首次那一次问。**和工具栏上那个复选框走同一条路**——
 *  另开一条就会出现「从按钮开的没存好」「从复选框开的没判重」这类
 *  只在某个入口下才有的问题 */
async function askWatchOnce(enable: boolean): Promise<void> {
  watchAsked = true;
  watchOn = enable;
  try {
    await api.setClipboardWatch(enable);
    if (enable) showToast(t("toast.watchOn"));
    // 关着的时候**什么都不说**。用户刚点了"不用",再弹一条"已关闭"
    // 就是在确认他的决定——他刚才那一下点得还不够明确吗
  } catch (err) {
    watchOn = !enable;
    watchAsked = false; // 没改成就不算问过,下次还能问
    showBackendError("error.internal", err);
  }
}

/** 开关剪贴板监控。设置面板的开关和首次询问走同一条路——两处各写
 *  一份,就会出现「面板里关不掉、首次询问能关」这种只在一边复现的问题 */
async function setWatchToggle(next: boolean): Promise<void> {
  watchOn = next;
  try {
    const info = await api.setClipboardWatch(next);
    watchOn = info.watching;
    if (info.watching) showToast(t("toast.watchOn"));
    else hideToast();
    syncSettingsPanel();
  } catch (err) {
    watchOn = !next; // 状态没改成,拨回去
    syncSettingsPanel();
    showError(wireText(err));
  }
}

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

/** 正文里的链接不许 WebView 自己处理。详情页是整个应用的界面,在
 *  这里点一条外链,默认行为是把 Quire 导航成那篇文章——软件就这样
 *  "没了"。所以所有 http(s) 链接统一下放给系统默认浏览器;锚点、
 *  相对路径这些页内的不拦。**挂在 document 上做委托**,正文每次重画
 *  都换一批节点,挨个绑监听既漏又忘解绑 */
document.addEventListener("click", (e) => {
  const target = e.target as HTMLElement | null;
  const anchor = target?.closest?.("a[href]");
  if (!anchor) return;
  const href = anchor.getAttribute("href") ?? "";
  if (!/^https?:\/\//i.test(href)) return;
  e.preventDefault();
  void api.openUrl(href).catch((err) => showError(wireText(err)));
});

document.addEventListener("keydown", (e) => {
  // Ctrl+V 分两种语境。光标在输入框/批注框里,粘贴就是往框里放内容,
  // 是输入框自己的本职;框外按 Ctrl+V 才是"把剪贴板里那篇存进来"。
  // 不加豁免的话,往批注里贴一段引用,引用没贴进去,库里反而多出
  // 一篇拿这段引用当正文的剪藏
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "v" && !isTyping(e.target)) {
    e.preventDefault();
    void pasteNow();
    return;
  }

  // F5 刷新。**得拦下默认行为**,否则 WebView 会把整个页面重新加载——
  // 用户的搜索词、选中的条目、正文滚动位置全没了,那不是刷新,那是重启
  if (e.key === "F5") {
    e.preventDefault();
    void refreshNow();
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
  // 抽屉开着的时候 Esc 先关抽屉。**Esc 应该是"退出当前这层"**,
  // 而不是一个全局的"什么都不是"——用户关掉抽屉之后还停在这儿,
  // 下一次按 Esc 他会以为程序没听见
  if (e.key === "Escape" && historyEl) {
    e.preventDefault();
    closeHistory();
    return;
  }
  if (e.key === "Escape" && shortcutPanelEl) {
    e.preventDefault();
    closeShortcuts();
    return;
  }
  if (e.key === "Escape" && statsPanelEl) {
    e.preventDefault();
    closeStats();
    return;
  }
  if (e.key === "Escape" && settingsPanelEl) {
    e.preventDefault();
    closeSettings();
    return;
  }
  if (e.key === "/" && !isTyping(e.target)) {
    e.preventDefault();
    searchEl.focus();
    searchEl.select();
    return;
  }
  // `?` 是问"这个软件怎么用"。**输入框里不拦**——用户打问号是在写字,
  // 不是在提问,拦下来他连中文标点都打不进去
  if (e.key === "?" && !isTyping(e.target)) {
    e.preventDefault();
    if (shortcutPanelEl) closeShortcuts();
    else openShortcuts();
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
  // 面板是动态搭的,`applyI18n` 扫不到它,得自己重画一遍
  syncSettingsPanel();
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
      // 回收站里的东西改不了,那儿的指纹没有意义,别拿它去写
      if (filter === "trash") renderTrashDetail(clip);
      else {
        activeStamp = clip.stamp ?? null;
        renderDetail(clip);
      }
    } catch (err) {
      if (filter !== "trash") showBackendError("error.readFailed", err);
    }
  }
}

refreshBtn.addEventListener("click", () => void refreshNow());

/**
 * 窗口重新拿到焦点就重扫。
 *
 * **这是"用户在外面改了文件"最常见的路径**:他在编辑器里补一段批注,
 * 切回 Quire,列表得是新的。没有这条,他得记得自己按 `F5`——而他不会记得。
 * 真上文件监控(`notify`)之前,这条是最划算的替代。
 *
 * **正在打字的时候不打断。** 焦点变化和"用户正在搜索框里输入"没有因果关系,
 * 一次重扫不至于毁掉什么,但搜索会重跑、结果会跳一下,那一下很烦人
 */
let focusRefreshTimer: ReturnType<typeof setTimeout> | null = null;
window.addEventListener("focus", () => {
  if (isTyping(document.activeElement)) return;
  // 去抖 300ms:切窗口时 focus 事件有时连着来两三次(窗口系统 + 应用自身),
  // 每来一次就全库扫一遍纯属白费
  if (focusRefreshTimer) clearTimeout(focusRefreshTimer);
  focusRefreshTimer = setTimeout(() => {
    focusRefreshTimer = null;
    void refreshNow();
  }, 300);
});


async function boot(): Promise<void> {
  // 先挂监听再拉列表。反过来的话,在这两步之间发生的剪藏不会触发任何事件,
  // 用户会看到"扩展显示剪藏成功,列表里却没有"
  await listen("clip-saved", () => void refreshList());
  // 剪藏是欠账**增加**的那一刻:图可能没下下来、正文可能没抓全。
  // 这一次不跟着 vault-changed 走——那个事件是外部改动引起的,不产生新欠账
  await listen("clip-saved", () => void refreshStatus());
  await listen("vault-changed", () => void refreshList());
  // 迁移搬了多少篇由这个事件带回来。**不在 `migrateVault` 的返回里**:
  // 返回值只有库信息,篇数得单独算,而算出来的那一刻才是真的搬完了
  await listen<VaultMigrated>("vault-migrated", (e) => {
    const { clips, assets, skipped } = e.payload;
    // 报的是**篇数**,图片跟在后面单独说。混成一个数字的话,两篇剪藏
    // 加二十几张图会报成「搬过去了 26 篇」——用户第一反应是
    // "我的库里怎么凭空多出这么多东西"
    showToast(
      skipped > 0
        ? t("settings.migrate.doneSkipped", {
            n: String(clips),
            assets: String(assets),
            skipped: String(skipped),
          })
        : t("settings.migrate.done", { n: String(clips), assets: String(assets) }),
    );
  });
  // 图片是后台下的,下完了才通知。这条提示是**特意要说出来的**:
  // Quire 一直说自己不联网,现在剪藏这一刻会真的去连图片服务器,
  // 悄悄做和写在脸上是两回事
  await listen<{ filename: string; count: number; failed: number }>("clip-saved-images", (e) => {
    // 没存下来的那几张**必须说出来**。那些图在 .md 里还指着 CDN,
    // 用户以为"数据都在本地",换设备才发现全废——那时候已经晚矣
    if (e.payload.failed > 0) {
      showToast(
        t("toast.savedImagesPartial", {
          count: e.payload.count,
          failed: e.payload.failed,
        }),
      );
      return;
    }
    showToast(t("toast.savedImages", { count: e.payload.count }));
  });
  // 监控开着却读不到剪贴板,用户不会怀疑监控,只会怀疑自己复制的东西有问题
  await listen("clipboard-watch-broken", () => {
    showError(t("watch.broken"));
  });
  await listen<ClipboardCapture>("clipboard-changed", (event) => {
    pendingCapture = event.payload;
    // **先记进历史,再弹提示。** 用户点了"忽略"之后内容就没了,
    // 而他很可能只是当时在忙。三分钟后想起来"刚才那个该存",没地方找——
    // 这是稍后读工具最常见的流失点:不是存不下,是错过之后找不回
    clipHistory.record({
      text: event.payload.text ?? "",
      html: event.payload.html ?? null,
      url: event.payload.url ?? null,
      meta: event.payload.meta ?? {},
      at: Date.now(),
    });
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
      {
        // 不叫"忽略"了。它确实已经记在历史里,用户随时能回来找。
        // 叫"忽略"是在骗他——他真以为这东西没存在过
        label: t("toast.ignore"),
        onClick: () => {
          hideToast();
          openHistory();
        },
      },
    ]);
  });

  await loadVaultInfo();
  await refreshList();
  // 启动这一次就够。**不跟 refreshList 走**——那个函数已经要读一遍全库,
  // 再叠一次状态查询就是白白翻倍,而欠账不会每次重扫都变
  void refreshStatus();
  renderTagBar();
  // 回到上次读的那篇。**读不出来就算了,回列表**——
  // 那一篇被删了、或者换了剪藏库,都是正常情况,不该拦住启动。
  // 记它的时候也**不重置计时器**:这次打开本来就是在续上次那一篇,
  // 再等 10 秒记一遍是白折腾一次磁盘
  const resume = await api.lastRead().catch(() => null);
  if (resume && clips.some((c) => c.filename === resume)) {
    await openDetail(resume);
  } else {
    renderEmptyDetail();
  }
}

// 兜底:任何"发射后不管"的 Promise 拒了,不能一声不响。
//
// `void someCall()` 这种写法满仓库都是(`void refreshList()`、`void boot()`、
// `void trashClip()`),好处是不用层层 await,代价是**拒了没人接**。
// 之前 `boot()` 里四个 `listen` 只要有一个因 IPC 异常拒了,后面的监听
// 全都不注册、列表永远空、剪藏后不刷新,而界面上**一条错误都没有**——
// 用户面对一个完全没反应的窗口,只会判定"软件坏了"。
//
// 这里兜住的是**漏网的那些**,不是用来替代各处自己的 catch:该就地处理的
// 错误仍然在原地报(比如批注存不上要告诉用户哪一段没存上),这条只是保证
// 没有哪个错误会彻底消失
window.addEventListener("unhandledrejection", (event) => {
  event.preventDefault();
  showError(t("error.unexpected", { detail: wireText(event.reason) }));
});

void boot();
