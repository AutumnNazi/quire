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

/** 懒加载图片的真地址藏在这些属性里,按出现频率排的。 */
const LAZY_SRC = ["data-src", "data-original", "data-lazy-src"];
const LAZY_SRCSET = ["data-srcset", "data-lazy-srcset"];

/** 占位图:1px 透明 gif 之类,八成以 data: 开头,或干脆没有。 */
function isPlaceholder(src: string | null): boolean {
  const s = (src ?? "").trim();
  return s === "" || s.startsWith("data:");
}

/**
 * 从 srcset 候选里挑分辨率最高的一段。
 *
 * srcset 的逗号可能出现在 CDN 的 URL 路径里(Substack 就这样),按逗号硬切
 * 会撕坏地址;所以按「空格分词、`Nw` 描述符结算」的方式读。懒加载页的
 * data-srcset 一般只有三五段,这个小解析器够用。
 */
function bestOfSrcset(srcset: string): string {
  let best = "";
  let bestWidth = 0;
  let pending: string[] = [];
  for (const token of srcset.trim().split(/\s+/)) {
    const width = token.match(/^(\d+)w,?$/);
    if (width) {
      const url = pending.join(" ").replace(/^,\s*/, "");
      if (url && pending.length > 0 && Number(width[1]) > bestWidth) {
        bestWidth = Number(width[1]);
        best = url;
      }
      pending = [];
    } else {
      pending.push(token);
    }
  }
  // 没有宽度描述符(如 "a.jpg 2x" 或裸 URL)就取最后一项兜底
  return best || pending[pending.length - 1] || "";
}

/** 相对地址在 Quire 里会对着自家协议解析成裂图,离开页面之前必须变绝对。 */
function absoluteOrRaw(raw: string, base: string): string {
  if (!base) return raw;
  try {
    return new URL(raw, base).toString();
  } catch {
    return raw;
  }
}

export interface MediaLabels {
  video: string;
  audio: string;
  embed: string;
  /** 流媒体(blob:)或动态注入的嵌入内容:本地播不了,链接指原文,
   *  文案要诚实写明「到原文观看」,别让用户以为链接坏了 */
  videoRemote: string;
  audioRemote: string;
  embedRemote: string;
}

/**
 * 找正文容器。**和 Defuddle 内部的入口选择器保持同一份**——搬媒体链接
 * 就是为了让它落在 Defuddle 认定的正文里,选择器对不上等于白搬。
 * (抄自 defuddle/dist/constants.js 的 ENTRY_POINT_ELEMENTS,去掉兜底的 body)
 */
const ENTRY_SELECTORS = [
  "#post",
  ".post-content",
  ".post-body",
  ".article-content",
  "#article-content",
  ".js-article-content",
  ".article_post",
  ".article-wrapper",
  ".entry-content",
  ".content-article",
  ".instapaper_body",
  ".post",
  ".markdown-body",
  "article",
  '[role="article"]',
  "main",
  '[role="main"]',
  ".article-body",
  "#content",
].join(", ");

/**
 * 埋点/统计类 iframe 的地址特征:新浪 sbeacon、百度统计 hm.baidu、
 * GA/analytics、各路像素跟踪。收录进正文就是一行垃圾链接,直接整个剔除。
 */
const TRACKING_SRC =
  /beacon|analytics|tracker|pixel|doubleclick|adservice|hm\.baidu|cnzz|umeng|clarity|hotjar|scorecardresearch|\/stats/i;

/**
 * 抽取前的正文预处理,修两个「图片视频没有链接」的根子。
 *
 * 1. **懒加载图片**:现代页面把真地址藏在 `data-src` 系列属性里,`src`
 *    只放 1px 占位。Defuddle 只认 `src`/`srcset`,拿不到就把整张图丢掉——
 *    正文里图片的位置直接消失。提升成 `src` 并把相对地址转绝对。
 * 2. **视频/音频/嵌入**:turndown 会把 `<video>`/`<iframe>` 原样保留成
 *    HTML,而 Quire 那边 `html:false` + DOMPurify 两道防线(刻意不拆,
 *    剪藏是不可信输入)只会把它们转义成一行字面代码或整个洗掉。换成
 *    Markdown 时代的通用答案——一条可点的链接。
 *
 * YouTube/Twitter 嵌入也一并换掉:Defuddle 自带的规则会把它们转成
 * `![](视频页面地址)` 的图片占位,在 Quire 里是必然裂掉的图。
 *
 * **不改动传进来的文档**——扩展那边传的是用户正看着的页面,改了会当着
 * 他的面闪变;返回的是克隆,改动全发生在克隆里。
 */
export function prepareDocumentForExtraction(
  source: Document,
  pageUrl: string,
  labels: MediaLabels,
): Document {
  const doc = source.cloneNode(true) as Document;

  for (const img of Array.from(doc.querySelectorAll("img"))) {
    if (isPlaceholder(img.getAttribute("src"))) {
      for (const attr of LAZY_SRC) {
        const value = img.getAttribute(attr)?.trim();
        if (value) {
          img.setAttribute("src", value);
          break;
        }
      }
      // 连 data-src 系列都没有的,从 data-srcset 里挑最大的一档
      if (isPlaceholder(img.getAttribute("src"))) {
        for (const attr of LAZY_SRCSET) {
          const srcset = img.getAttribute(attr)?.trim();
          if (srcset) {
            const best = bestOfSrcset(srcset);
            if (best) img.setAttribute("src", best);
            break;
          }
        }
      }
    }
    const src = img.getAttribute("src");
    if (src) img.setAttribute("src", absoluteOrRaw(src.trim(), pageUrl));
    // srcset 里全是没经过绝对化的候选,留着只会让 Defuddle 从里面拿到
    // 相对地址;真正的图已经在 src 里了
    img.removeAttribute("srcset");
    for (const attr of [...LAZY_SRC, ...LAZY_SRCSET]) img.removeAttribute(attr);
  }

  const relocated: HTMLAnchorElement[] = [];
  for (const el of Array.from(doc.querySelectorAll("video, audio, iframe"))) {
    const raw =
      el.getAttribute("src")?.trim() ||
      el.querySelector("source")?.getAttribute("src")?.trim() ||
      "";
    // 埋点/统计 iframe 不是内容,直接剔除,连链接都不留。
    // **没有 src 的 iframe 不能跟着剔**:新浪播放器的 src 是 JS 稍后
    // 填充的,初始为空——剔了它,视频入口跟着消失(真踩过)
    if (el.tagName === "IFRAME" && raw && TRACKING_SRC.test(raw)) {
      el.remove();
      continue;
    }
    // blob: 地址只在本页会话内有效(MSE 流媒体播放器的常态,新浪这类
    // JS 注入播放器的新闻页全是它),存成 Markdown 链接就是死链——
    // 取不到可用地址时链接指向原文页,想看视频点回去,别给假入口
    const usable = raw && !raw.startsWith("blob:");
    const href = usable ? absoluteOrRaw(raw, pageUrl) : pageUrl;
    const link = doc.createElement("a");
    link.setAttribute("href", href);
    // 能直连的用普通文案;blob:/空的只能回原文看,文案如实说
    link.textContent =
      el.tagName === "VIDEO"
        ? usable
          ? labels.video
          : labels.videoRemote
        : el.tagName === "AUDIO"
          ? usable
            ? labels.audio
            : labels.audioRemote
          : usable
            ? labels.embed
            : labels.embedRemote;
    el.replaceWith(link);
    relocated.push(link);
  }

  // 正文容器外的媒体链接,Defuddle 的正文选择会整个剔掉——新浪这类
  // 视频新闻页,播放器在正文容器上方,转出的链接留在原位等于不存在。
  // 搬进正文容器开头。容器找不到(散落式页面)就不硬塞:塞进 Defuddle
  // 不认的位置和没塞一样,还多一层干扰
  const entry = doc.querySelector(ENTRY_SELECTORS);
  if (entry) {
    const outside = relocated.filter((link) => !entry.contains(link));
    // prepend 逐个插到最前,后插的排前面——逆序才保得住文档顺序
    entry.prepend(...outside.reverse());
  }

  return doc;
}
