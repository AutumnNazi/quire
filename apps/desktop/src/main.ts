import "./style.css";
import { listen } from "@tauri-apps/api/event";
import { api } from "./api";
import type { ClipboardCapture } from "./clipboard";
import { clipToMarkdown, MARKDOWN_PLACEHOLDER, renderMarkdown } from "./markdown";
import { groupByWeek, selectClips, type ListMode } from "./list";
import type { ClipContent, ClipSummary, SearchHit, VaultInfo } from "./types";

const root = document.querySelector<HTMLDivElement>("#app");
if (!root) throw new Error("页面缺少 #app 挂载点");

root.innerHTML = `
  <header class="toolbar">
    <button class="btn primary" id="btn-paste" title="把剪贴板里的内容存成 Markdown(Ctrl+V)">粘贴剪藏</button>
    <span class="brand">Quire</span>
    <span class="vault-path" id="vault-path" title="剪藏目录"></span>
    <span class="spacer"></span>
    <input
      class="search"
      id="search"
      type="search"
      placeholder="搜索剪藏…"
      autocomplete="off"
      spellcheck="false"
      title="搜标题和正文。中文两字就能搜(比如「苹果」)。"
    />
    <div class="filters" role="tablist" aria-label="剪藏筛选">
      <button class="filter active" id="filter-all" role="tab" aria-selected="true">全部</button>
      <button class="filter" id="filter-unread" role="tab" aria-selected="false" title="没读过、也没归档的">未读</button>
      <button class="filter" id="filter-week" role="tab" aria-selected="false" title="按自然周分组,一眼看出这周积了多少">每周</button>
    </div>
    <label class="watch-toggle" title="开启后,你在别处复制文章时会自动提示存到 Quire。默认关闭。">
      <input type="checkbox" id="chk-watch" />
      <span>监控剪贴板</span>
    </label>
    <button class="btn" id="btn-export" title="把整个剪藏库导出成一个 Markdown 文件,Obsidian / Logseq 都能直接打开">导出</button>
    <button class="btn" id="btn-open">打开剪藏目录</button>
    <button class="btn" id="btn-pick">更换目录</button>
  </header>
  <main class="split">
    <aside class="list-pane">
      <div class="warn" id="warn" hidden></div>
      <div class="list" id="list"></div>
    </aside>
    <section class="detail-pane" id="detail"></section>
  </main>
  <div class="toast" id="toast" hidden>
    <span class="toast-text" id="toast-text"></span>
    <span class="toast-actions" id="toast-actions" hidden>
      <button class="btn primary" id="toast-save">保存</button>
      <button class="btn" id="toast-dismiss">忽略</button>
    </span>
    <button class="btn close" id="toast-close" aria-label="关闭">×</button>
  </div>
`;

const el = <T extends HTMLElement>(id: string): T => {
  const node = document.getElementById(id);
  if (!node) throw new Error(`缺少节点 #${id}`);
  return node as T;
};

const listEl = el<HTMLDivElement>("list");
const detailEl = el<HTMLElement>("detail");
const warnEl = el<HTMLDivElement>("warn");
const vaultPathEl = el<HTMLSpanElement>("vault-path");
const toastEl = el<HTMLDivElement>("toast");
const toastTextEl = el<HTMLSpanElement>("toast-text");
const toastActionsEl = el<HTMLSpanElement>("toast-actions");
const watchEl = el<HTMLInputElement>("chk-watch");
const searchEl = el<HTMLInputElement>("search");
const filterAllEl = el<HTMLButtonElement>("filter-all");
const filterUnreadEl = el<HTMLButtonElement>("filter-unread");
const filterWeekEl = el<HTMLButtonElement>("filter-week");

let clips: ClipSummary[] = [];
/** 非空时列表显示的是检索结果,而不是全库。空数组和 null 要分清:
 *  null = 没在搜,空数组 = 搜了但一条没中,两者界面不一样。 */
let hits: SearchHit[] | null = null;
let activeFilename: string | null = null;
/** 当前筛选。搜索和筛选是**两回事**:搜出来的结果不再过筛,否则用户
 *  搜到一篇却看不见,只会当成搜索坏了。 */
let filter: ListMode = "all";
/** 监控弹出来的内容。等用户点"保存"时才真正落盘——自动存等于替他做决定。 */
let pendingCapture: ClipboardCapture | null = null;
let toastTimer: ReturnType<typeof setTimeout> | null = null;
let searchTimer: ReturnType<typeof setTimeout> | null = null;

function showError(message: string): void {
  warnEl.textContent = message;
  warnEl.hidden = false;
}

function clearError(): void {
  warnEl.hidden = true;
  warnEl.textContent = "";
}

function showToast(text: string, withActions: boolean): void {
  if (toastTimer) clearTimeout(toastTimer);
  toastTextEl.textContent = text;
  toastActionsEl.hidden = !withActions;
  toastEl.hidden = false;
  if (!withActions) {
    // 纯提示两三秒后自己消失;带按钮的等用户处理,不自动消失
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

function renderList(): void {
  listEl.replaceChildren();

  if (hits) {
    if (hits.length === 0) {
      const none = document.createElement("div");
      none.className = "empty";
      none.innerHTML = `<p class="empty-title">没找到</p><p>换个词试试。</p>`;
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
      <p class="empty-title">剪藏库还是空的</p>
      <p>在任意文章页里选中正文,按 <kbd>Ctrl</kbd>+<kbd>C</kbd> 复制,<br />再回到 Quire 按 <kbd>Ctrl</kbd>+<kbd>V</kbd> 就能存进来。</p>
    `;
    listEl.append(empty);
    return;
  }

  // 「每周」单独走分组渲染。其余两种筛选都是同一份列表,只是筛得不一样。
  if (filter === "week") {
    renderWeekList();
    return;
  }

  const shown = selectClips(clips, filter);
  if (shown.length === 0) {
    const none = document.createElement("div");
    none.className = "empty";
    // 说清楚为什么空:是"还没读"还是"都读完了",差别很大。
    // 只写"没有内容"的话,用户会以为剪藏丢了。
    none.innerHTML = filter === "unread"
      ? `<p class="empty-title">没有未读了</p><p>都读过了。要看全部,点「全部」。</p>`
      : `<p class="empty-title">没有剪藏</p>`;
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
    none.innerHTML = `<p class="empty-title">没有可回顾的剪藏</p>`;
    listEl.append(none);
    return;
  }
  for (const group of groups) {
    const header = document.createElement("div");
    header.className = "week-header";
    const unread = group.clips.filter((c) => !c.read && !c.archived).length;
    // 整组都读完了就别再报未读,挂个 0 只会让人多看一眼
    const suffix = unread > 0 ? ` · 还有 ${unread} 篇没读` : " · 都读完了";
    header.textContent = `${group.isoYear} 年第 ${group.isoWeek} 周 — ${group.clips.length} 篇${suffix}`;
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
    listEl.append(clipItem(hit.summary, hit.snippet));
  }
}

function clipItem(clip: ClipSummary, sub: string | null): HTMLElement {
  const item = document.createElement("article");
  item.className = "clip";
  if (clip.filename === activeFilename) item.classList.add("active");
  if (clip.read || clip.archived) item.classList.add("done");

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

  // 已读开关。**不自动标已读**——打开列表就把一堆剪藏刷成已读,
  // 那等于替用户做决定,而且不可逆(他可能只是误点了一下)。
  const toggle = document.createElement("button");
  toggle.className = "read-toggle";
  const done = clip.read || clip.archived;
  toggle.textContent = done ? "已读" : "标已读";
  toggle.title = done ? "点一下标回未读" : "读完了,点一下标已读";
  toggle.setAttribute("aria-pressed", String(done));
  toggle.addEventListener("click", (event) => {
    // 不冒泡:不然会连带触发下面的 openDetail,读一篇却把列表刷没了
    event.stopPropagation();
    void toggleRead(clip, !clip.read);
  });
  item.append(toggle);

  item.addEventListener("click", () => void openDetail(clip.filename));
  return item;
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
    showError(`标已读失败:${String(err)}`);
  }
}

function renderDetail(clip: ClipContent): void {
  detailEl.replaceChildren();

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
    link.textContent = "打开原文";
    meta.prepend(link);
  }

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

  header.append(title, meta);
  detailEl.append(header, body);
  detailEl.scrollTop = 0;
}

function renderEmptyDetail(): void {
  detailEl.replaceChildren();
  const hint = document.createElement("div");
  hint.className = "empty";
  hint.innerHTML = `<p class="empty-title">从左边选一篇</p>`;
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
    showError(`读不出来:${String(err)}`);
  }
}

async function saveCapture(capture: ClipboardCapture): Promise<void> {
  const { markdown, title, excerpt, meta } = clipToMarkdown(capture);
  if (!markdown.trim()) {
    showError("剪贴板里没有可保存的内容");
    return;
  }
  await api.saveClip({
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
  });
  clearError();
}

async function pasteNow(): Promise<void> {
  try {
    const capture = await api.captureClipboard();
    if (!capture.html?.trim() && !capture.text.trim()) {
      showError("剪贴板是空的,先在别处复制点内容");
      return;
    }
    await saveCapture(capture);
    showToast("已剪藏", false);
  } catch (err) {
    showError(String(err));
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
    const results = await api.searchClips(trimmed);
    // 用户可能已经改词或清空了。这次的返回值过期,丢掉
    if (searchEl.value.trim() !== trimmed) return;
    hits = results;
    renderList();
  } catch (err) {
    showError(`搜不了:${String(err)}`);
  }
}

// 每敲一下就全库扫一遍,剪藏多了会跟着手抖。去抖 200ms 是体感不明显的下限。
searchEl.addEventListener("input", () => {
  if (searchTimer) clearTimeout(searchTimer);
  const value = searchEl.value;
  searchTimer = setTimeout(() => void runSearch(value), 200);
});

async function refreshList(): Promise<void> {
  try {
    const result = await api.listClips();
    clips = result.clips;

    if (result.unreadable.length > 0) {
      // 这些文件确实存在但读不出元数据。藏起来等于骗用户"剪藏丢了",
      // 摆出来用户自己能看到是哪个文件出了问题
      showError(
        `有 ${result.unreadable.length} 个文件读不出元数据:` +
          result.unreadable.map((f) => f.filename).join("、"),
      );
    } else {
      clearError();
    }

    renderList();
  } catch (err) {
    showError(`列不出剪藏:${String(err)}`);
  }
}

async function loadVaultInfo(): Promise<void> {
  try {
    const info: VaultInfo = await api.vaultInfo();
    vaultPathEl.textContent = info.path;
    vaultPathEl.title = info.path;
    watchEl.checked = info.watching;
  } catch {
    vaultPathEl.textContent = "剪藏目录未知";
  }
}

el<HTMLButtonElement>("btn-paste").addEventListener("click", () => void pasteNow());

el<HTMLButtonElement>("btn-open").addEventListener("click", () => {
  void api.openVaultFolder().catch((e) => showError(String(e)));
});

/** 切筛选。搜索态下不切——搜出来的结果和自己的筛选无关,
 *  硬切会让用户以为"搜到的东西被筛没了",是搜索坏了。 */
function setFilter(next: ListMode): void {
  filter = next;
  for (const [button, value] of [
    [filterAllEl, "all"],
    [filterUnreadEl, "unread"],
    [filterWeekEl, "week"],
  ] as const) {
    const on = value === next;
    button.classList.toggle("active", on);
    button.setAttribute("aria-selected", String(on));
  }
  renderList();
}
filterAllEl.addEventListener("click", () => setFilter("all"));
filterUnreadEl.addEventListener("click", () => setFilter("unread"));
filterWeekEl.addEventListener("click", () => setFilter("week"));

el<HTMLButtonElement>("btn-export").addEventListener("click", async () => {
  try {
    // 返回 null 是用户在保存对话框点了取消,那不是故障,别弹红字
    const path = await api.exportVault();
    if (path) showToast("已导出", false);
  } catch (err) {
    showError(`导出失败:${String(err)}`);
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
    showError(`更换目录失败:${String(err)}`);
  }
});

watchEl.addEventListener("change", () => {
  void api
    .setClipboardWatch(watchEl.checked)
    .then((info) => {
      watchEl.checked = info.watching;
      if (info.watching) {
        showToast("已开启监控:复制文章后会提示保存", false);
      } else {
        hideToast();
      }
    })
    .catch((err) => {
      watchEl.checked = !watchEl.checked; // 状态没改成,把开关拨回去
      showError(String(err));
    });
});

// Ctrl+V 走的是系统剪贴板,不是往 DOM 里插文本,所以要拦下默认行为
document.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "v") {
    e.preventDefault();
    void pasteNow();
  }
});

el<HTMLButtonElement>("toast-close").addEventListener("click", hideToast);
el<HTMLButtonElement>("toast-dismiss").addEventListener("click", hideToast);
el<HTMLButtonElement>("toast-save").addEventListener("click", () => {
  if (!pendingCapture) {
    hideToast();
    return;
  }
  const capture = pendingCapture;
  hideToast();
  void saveCapture(capture)
    .then(() => showToast("已剪藏", false))
    .catch((err) => showError(String(err)));
});

async function boot(): Promise<void> {
  // 先挂监听再拉列表。反过来的话,在这两步之间发生的剪藏不会触发任何事件,
  // 用户会看到"扩展显示剪藏成功,列表里却没有"
  await listen("clip-saved", () => void refreshList());
  await listen("vault-changed", () => void refreshList());
  await listen<ClipboardCapture>("clipboard-changed", (event) => {
    pendingCapture = event.payload;
    const preview =
      event.payload.text?.trim().split("\n").find((l) => l.trim())?.slice(0, 40) || "剪贴板内容";
    showToast(`检测到:${preview}`, true);
  });

  await loadVaultInfo();
  await refreshList();
  renderEmptyDetail();
}

void boot();
