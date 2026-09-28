/**
 * 把 Defuddle 的抽取结果拼成往剪贴板里写的那份数据。
 *
 * 从 content.ts 里拆出来是为了能测:content.ts 顶层就往 chrome.runtime 上
 * 挂监听,在 Node 里 import 就会炸。拆开之后这段纯逻辑能直接跑测试。
 *
 * 这里的输出就是与桌面端之间的全部约定——改任何一个键名,都得同步改
 * Rust 侧的 `EXTENSION_META`,不然用户装完发现"字段还是空的"。
 */

/** 与 Rust 侧 `EXTENSION_META` 一一对应。 */
export const META = {
  version: "quire-version",
  title: "quire-title",
  site: "quire-site",
  author: "quire-author",
  excerpt: "quire-excerpt",
  published: "quire-published",
  image: "quire-image",
} as const;

/** 协议版本。桌面端靠 `quire-version` 判断"这份剪贴板是扩展写的"。 */
export const PROTOCOL_VERSION = "1";

/** Defuddle 的解析结果里我们真正用到的那几个字段。 */
export interface Extracted {
  title?: string;
  site?: string;
  domain?: string;
  author?: string;
  description?: string;
  published?: string;
  image?: string;
}

export interface ClipPayload {
  /** 给 Quire 用的 Markdown,写进 text/plain */
  markdown: string;
  /** CF_HTML 的 context 段,写进 text/html */
  html: string;
  /** 附带的元数据键值,只含非空项 */
  metas: Array<[string, string]>;
}

/**
 * 挑一个能看的站点名。
 *
 * Defuddle 的 `site` 在页面缺 `og:site_name` 时会**退化成作者名**,直接用
 * 就会出现"站点:张三"。这种情况退回域名——至少是个真名字。
 */
export function pickSite(
  site: string | undefined,
  author: string,
  domain: string | undefined,
  fallback: string,
): string {
  const name = (site ?? "").trim();
  if (name && name !== author) return name;
  return (domain ?? "").trim() || fallback;
}

export function escapeAttr(value: string): string {
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
export function buildHtml(
  content: string,
  metas: Array<[string, string]>,
  url: string,
): string {
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
 * 组装最终载荷。
 *
 * 空值一律不写。桌面端收到空字符串还得挨个判空,不如压根别发。
 */
export function buildPayload(
  extracted: Extracted,
  options: { content: string; markdown: string; url: string; fallbackTitle: string },
): ClipPayload {
  const author = (extracted.author ?? "").trim();
  const title = (extracted.title ?? "").trim() || options.fallbackTitle;
  const pairs: Array<[string, string]> = [
    [META.version, PROTOCOL_VERSION],
    [META.title, title],
    [
      META.site,
      pickSite(extracted.site, author, extracted.domain, safeHost(options.url)),
    ],
    [META.author, author],
    // Defuddle 里摘要字段叫 description,不叫 excerpt
    [META.excerpt, (extracted.description ?? "").trim()],
    [META.published, (extracted.published ?? "").trim()],
    [META.image, (extracted.image ?? "").trim()],
  ];

  return {
    markdown: options.markdown,
    html: buildHtml(options.content, pairs.filter(([, v]) => v.length > 0), options.url),
    metas: pairs.filter(([, v]) => v.length > 0),
  };
}

/** 拿不准 URL 的时候就退回到一个空串,别把坏地址塞进 frontmatter。 */
function safeHost(url: string): string {
  try {
    return new URL(url).hostname;
  } catch {
    return "";
  }
}
