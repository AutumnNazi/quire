import "./style.css";
import { listen } from "@tauri-apps/api/event";
import { api } from "./api";
import type { ClipboardCapture } from "./clipboard";
import { clipToMarkdown, MARKDOWN_PLACEHOLDER, renderMarkdown } from "./markdown";
import type { ClipContent, ClipSummary, VaultInfo } from "./types";

const root = document.querySelector<HTMLDivElement>("#app");
if (!root) throw new Error("页面缺少 #app 挂载点");

root.innerHTML = `
  <header class="toolbar">
    <button class="btn primary" id="btn-paste" title="把剪贴板里的内容存成 Markdown(Ctrl+V)">粘贴剪藏</button>
    <span class="brand">Quire</span>
    <span class="vault-path" id="vault-path" title="剪藏目录"></span>
    <span class="spacer"></span>
    <label class="watch-toggle" title="开启后,你在别处复制文章时会自动提示存到 Quire。默认关闭。">
      <input type="checkbox" id="chk-watch" />
      <span>监控剪贴板</span>
    </label>
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

let clips: ClipSummary[] = [];
let activeFilename: string | null = null;
/** 监控弹出来的内容。等用户点"保存"时才真正落盘——自动存等于替他做决定。 */
let pendingCapture: ClipboardCapture | null = null;
let toastTimer: ReturnType<typeof setTimeout> | null = null;

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

  for (const clip of clips) {
    const item = document.createElement("article");
    item.className = "clip";
    if (clip.filename === activeFilename) item.classList.add("active");

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
    if (clip.excerpt) {
      const excerpt = document.createElement("p");
      excerpt.className = "clip-excerpt";
      excerpt.textContent = clip.excerpt;
      item.append(excerpt);
    }

    item.addEventListener("click", () => void openDetail(clip.filename));
    listEl.append(item);
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
