import Defuddle from "defuddle";

/**
 * 把整篇文章连同元数据写进剪贴板,交给 Quire 桌面端。
 *
 * 为什么不直接发到桌面端:那需要在本机开一个监听端口,等于给同浏览器的
 * 恶意网页留了个可攻击面。剪贴板是操作系统本来就有的通道,用现成的
 * 链路不新增任何监听端口,也不用再弹一次权限框。
 *
 * **这个扩展是可选的。** 没装它,Quire 照常工作,只是拿不到整页正文和
 * 页面元数据,只能用用户划选的那一小块。
 */

/** 与 Rust 侧 `EXTENSION_META` 一一对应。改这边必须同步改那边——
 *  两边键名对不上不会有任何编译错误,只会让用户装完发现"字段还是空的"。 */
const META = {
  version: "quire-version",
  title: "quire-title",
  site: "quire-site",
  author: "quire-author",
  excerpt: "quire-excerpt",
  published: "quire-published",
  image: "quire-image",
} as const;

/** 协议版本。桌面端靠 `quire-version` 判断"这份剪贴板是扩展写的"。 */
const PROTOCOL_VERSION = "1";

interface ClipRequest {
  type: "quire-clip";
}

interface ClipReply {
  ok: boolean;
  title?: string;
  error?: string;
}

chrome.runtime.onMessage.addListener((message: unknown, _sender, sendResponse) => {
  const request = message as ClipRequest | undefined;
  if (request?.type !== "quire-clip") return undefined;

  try {
    sendResponse(clipCurrentPage());
  } catch (err) {
    sendResponse({ ok: false, error: err instanceof Error ? err.message : String(err) });
  }
  // 同步回话,不用 return true 占住通道
  return undefined;
});

function clipCurrentPage(): ClipReply {
  // separateMarkdown 一次给出两份:content 是干净 HTML,contentMarkdown 是
  // Markdown。**不能换成 markdown: true**——那样 content 变 Markdown,
  // 就拿不到给别的应用用的 HTML 了,两个一起开更会互相打架。
  const result = new Defuddle(document, {
    url: location.href,
    separateMarkdown: true,
  }).parse();

  const markdown = (result.contentMarkdown ?? "").trim();
  if (!markdown) {
    // 登录页、搜索结果页、纯图片站都走这条路
    return { ok: false, error: "这一页抽不出正文,可能是登录页或纯图片页" };
  }

  const title = (result.title ?? "").trim() || document.title || location.hostname;
  const author = (result.author ?? "").trim();
  const pairs: Array<[string, string]> = [
    [META.version, PROTOCOL_VERSION],
    [META.title, title],
    [META.site, pickSite(result.site, author, result.domain)],
    [META.author, author],
    // Defuddle 里摘要字段叫 description,不叫 excerpt
    [META.excerpt, (result.description ?? "").trim()],
    [META.published, (result.published ?? "").trim()],
    [META.image, (result.image ?? "").trim()],
  ];
  // 空值一律不写。桌面端那边收到空字符串还得挨个判空,不如压根别发
  const metas = pairs.filter(([, value]) => value.length > 0);

  const html = buildHtml(result.content, metas, location.href);
  if (!writeToClipboard(html, markdown)) {
    return { ok: false, error: "浏览器拒绝了写入剪贴板" };
  }
  return { ok: true, title };
}

/**
 * 挑一个能看的站点名。
 *
 * Defuddle 的 `site` 在页面缺 `og:site_name` 时会**退化成作者名**,直接用
 * 就会出现"站点:张三"。这种情况退回域名——至少是个真名字。
 */
function pickSite(site: string | undefined, author: string, domain: string | undefined): string {
  const name = (site ?? "").trim();
  if (name && name !== author) return name;
  return (domain ?? "").trim() || location.hostname;
}

function escapeAttr(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/**
 * 拼一份 CF_HTML 风格的 context 段。
 *
 * 正文是给人看的那份干净 HTML;元数据挂在 `<meta>` 上,由桌面端读。
 * `source-url` 是桌面端认的四种取址位置之一——这里用的就是它。
 */
function buildHtml(content: string, metas: Array<[string, string]>, url: string): string {
  const tags = metas
    .map(([name, value]) => `<meta name="${name}" content="${escapeAttr(value)}">`)
    .join("");
  const sourceUrl = `<meta name="source-url" content="${escapeAttr(url)}">`;
  return (
    `<html><head><meta charset="utf-8">${tags}${sourceUrl}</head>` +
    `<body><!--StartFragment-->${content}<!--EndFragment--></body></html>`
  );
}

/**
 * 写剪贴板。
 *
 * 走 `execCommand("copy")` 而不是 `navigator.clipboard.write`:后者要求
 * 文档处在用户手势上下文里,从 service worker 发起的调用会被拒。前者
 * 只要页面本身有焦点就能成,而点扩展图标并不会把焦点从网页上拿走。
 */
function writeToClipboard(html: string, text: string): boolean {
  const onCopy = (event: ClipboardEvent) => {
    // 不 preventDefault 的话浏览器会拿页面选中内容盖掉我们写的数据
    event.preventDefault();
    event.clipboardData?.setData("text/html", html);
    event.clipboardData?.setData("text/plain", text);
  };

  document.addEventListener("copy", onCopy);
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  } finally {
    document.removeEventListener("copy", onCopy);
  }
  return ok;
}
