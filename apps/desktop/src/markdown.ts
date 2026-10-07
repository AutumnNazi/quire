import MarkdownIt from "markdown-it";
import DOMPurify from "dompurify";
// 必须从 /full 入口导入。主入口("defuddle")里根本没有接上 Markdown 转换——
// 它的 defuddle.js 里 toMarkdown 一次都不出现。后果不是报错,是**静默失效**:
// markdown: true 被无视,content 照旧是 HTML,于是剪藏存进 .md 的就是 HTML。
// 扩展那条路更惨:separateMarkdown 在主入口下 contentMarkdown 恒为 undefined,
// 直接判"抽不出正文",每次都失败。
import Defuddle from "defuddle/full";
import type { ClipboardCapture } from "./clipboard";
import { t } from "./i18n";

/**
 * 剪藏内容全部来自第三方网页,当不可信输入处理。
 *
 * 两道防线缺一不可:`html:false` 让 markdown 里的原始 HTML 不参与解析,
 * DOMPurify 再兜住解析器本身的疏漏。只靠其中一道都不够——前者挡不住
 * markdown-it 生成的 `<a href="javascript:...">`,后者才是最终保证。
 */
const md = new MarkdownIt({ html: false, linkify: true, breaks: false });

const defaultLinkOpen =
  md.renderer.rules.link_open ??
  ((tokens, idx, options, _env, self) => self.renderToken(tokens, idx, options));

md.renderer.rules.link_open = (tokens, idx, options, env, self) => {
  // 跳出去的链接一律新标签打开并断掉 opener,否则目标页面能反向操作 Quire 窗口
  tokens[idx].attrSet("target", "_blank");
  tokens[idx].attrSet("rel", "noopener noreferrer");
  return defaultLinkOpen(tokens, idx, options, env, self);
};

export function renderMarkdown(source: string): string {
  return DOMPurify.sanitize(md.render(source), {
    USE_PROFILES: { html: true },
    // 默认的 html profile 会把 target 洗掉,于是 link_open 里加的
    // target="_blank" 到不了最终 DOM,外链全在当前标签页开。得显式加回来。
    // 安全性不受影响:markdown 侧 html:false 已经把外来 HTML 全转义了,
    // 活到这里的标签只有 markdown-it 自己生成的,而 rel=noopener 也照样加。
    ADD_ATTR: ["target"],
  });
}

/** 图片挂不上时给个占位,免得列表里出现一串碎图标。 */
export const MARKDOWN_PLACEHOLDER =
  "data:image/svg+xml;charset=utf-8," +
  encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">
       <rect width="16" height="16" rx="3" fill="#e6e6e6"/>
       <path d="M3 11l3-3 2 2 2-2 3 3z" fill="#b0b0b0"/>
       <circle cx="6" cy="5.5" r="1.3" fill="#b0b0b0"/>
     </svg>`,
  );

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
 * 会撕坏地址;所以按「空格分词、`Nw` 描述符结算」的方式读。
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
  return best || pending[pending.length - 1] || "";
}

/** 相对地址在 Quire 里会对着自家协议解析成裂图,落盘之前必须变绝对。 */
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
 * Defuddle 之前的正文预处理,与扩展侧 extract.ts 里的同名逻辑一致
 * (跨包不共享代码,两边各有测试锁行为——改一边必须同步改另一边)。
 *
 * Defuddle 自己会认 data-src、data-srcset 并绝对化相对地址(探针实测),
 * 这里补它不认的 `data-original`/`data-lazy-src` 变体,再把视频/音频/
 * iframe 换成链接——它们会被 Defuddle 原样保留成 HTML,而渲染侧
 * html:false + DOMPurify 两道防线(刻意不拆,剪藏是不可信输入)只会把
 * 它们洗成一行字面代码。YouTube 嵌入也一并换:Defuddle 自带规则会把它
 * 转成 `![](视频页面地址)` 的图片,在 Quire 里是必然裂掉的图。
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

/**
 * 把正文里**本地化过**的相对图片地址(`assets/<id>/0.png`)换成 WebView
 * 读得到的 asset 地址。
 *
 * 图片本地化把远程图下到库目录并重写正文 URL,但相对路径落在 WebView 里
 * 是相对页面本身的(`tauri://localhost/assets/...`),必然 404——下载越
 * 成功裂图越彻底。库根路径 + asset 白名单(Rust 侧运行时挂)凑齐才能读。
 *
 * 只动 `assets/` 开头的;远程图、data: 图不是本地化的产物,原样保留。
 * 没有库路径(库还没就绪)时不转换,裂归裂,别把地址改坏。
 * `toAssetUrl` 由调用方注入(Tauri 的 convertFileSrc):markdown.ts 不沾
 * Tauri 运行时,jsdom 里测得了。
 */
export function resolveAssetImages(
  container: HTMLElement,
  vaultPath: string,
  toAssetUrl: (absolutePath: string) => string,
): void {
  const base = vaultPath.trim().replace(/[\\/]+$/, "");
  if (!base) return;
  for (const img of Array.from(
    container.querySelectorAll<HTMLImageElement>('img[src^="assets/"]'),
  )) {
    const src = img.getAttribute("src") ?? "";
    img.src = toAssetUrl(`${base}/${src}`);
  }
}

/** 剪贴板转出来的结果。 */
export interface ClipMarkdown {
  markdown: string;
  title: string;
  excerpt: string;
  /** 扩展带来的页面级元数据。没装扩展就是空对象。 */
  meta: Record<string, string>;
}

/**
 * 把剪贴板内容转成 Markdown。
 *
 * 分两条路:
 * - **装了扩展**:扩展已经对着整页跑过一遍 Defuddle,Markdown 就躺在纯文本里。
 *   这时直接用,不再过第二遍机器——干净内容被再加工一次,只会多一处出错的机会。
 * - **没装扩展**(默认路径):只有用户划选的那一小块 HTML,交给 Defuddle 抽。
 *
 * 任何一级失败都不能把内容丢了——用户复制过来的东西再难看也是他主动选的,
 * 总好过点一次「保存」得到一个空文件。
 */
export function clipToMarkdown(capture: ClipboardCapture): ClipMarkdown {
  const meta = capture.meta ?? {};
  const fromExtension = Boolean(meta["quire-version"]);
  const extTitle = meta["quire-title"]?.trim();

  if (fromExtension && capture.text.trim()) {
    return {
      markdown: capture.text,
      title: extTitle || firstLine(capture.text),
      excerpt: meta["quire-excerpt"]?.trim() || excerptOf(capture.text),
      meta,
    };
  }

  if (capture.html?.trim()) {
    try {
      const doc = new DOMParser().parseFromString(capture.html, "text/html");
      // DOMParser 产的是私有文档,直接改;扩展路径的预处理在扩展那边做
      const prepared = prepareDocumentForExtraction(doc, capture.url ?? "", {
        video: t("clip.mediaVideo"),
        audio: t("clip.mediaAudio"),
        embed: t("clip.mediaEmbed"),
        videoRemote: t("clip.mediaVideoRemote"),
        audioRemote: t("clip.mediaAudioRemote"),
        embedRemote: t("clip.mediaEmbedRemote"),
      });
      const defuddle = new Defuddle(prepared, {
        url: capture.url ?? undefined,
        // 只开 markdown: 它把 content 直接转成 Markdown。与 separateMarkdown
        // 互斥,两个一起开反而会让 contentMarkdown 永远是 undefined
        markdown: true,
      });
      const result = defuddle.parse();
      if (result.content?.trim()) {
        return {
          markdown: result.content,
          title: extTitle || pickTitle(result.title, doc),
          excerpt: meta["quire-excerpt"]?.trim() || excerptOf(result.content),
          meta,
        };
      }
    } catch {
      // 落到下面的纯文本兜底
    }
  }
  return {
    markdown: capture.text,
    title: firstLine(capture.text),
    excerpt: excerptOf(capture.text),
    meta,
  };
}

/** Defuddle 给的标题可能为空,补一次文档里的首个标题。 */
function pickTitle(fromDefuddle: string | undefined, doc: Document): string {
  if (fromDefuddle?.trim()) return fromDefuddle.trim();
  const heading = doc.querySelector("h1, h2, h3")?.textContent?.trim();
  if (heading) return heading;
  return doc.title?.trim() || "";
}

/** 纯文本兜底的标题:第一行非空内容,截到 80 字。 */
function firstLine(text: string): string {
  const line = text
    .split("\n")
    .map((s) => s.trim())
    .find((s) => s.length > 0);
  return line ? line.slice(0, 80) : "";
}

/**
 * 从 Markdown 正文里取一段能当摘要的纯文本。
 *
 * 没装扩展时剪贴板里根本没有页面摘要,而列表的副标题、以后搜索结果的
 * 上下文都指望这个字段。从正文里捞第一段像样的文字,比让字段永远空着强。
 * 标题、代码块、引用、列表、图片都不适合当摘要,跳过。
 */
function excerptOf(markdown: string, limit = 120): string {
  const block = markdown
    .split(/\n{2,}/)
    .map((b) => b.trim())
    .find((b) => b.length > 0 && !/^[#>|`\-*+\]]/.test(b));
  if (!block) return "";

  const text = block
    .replace(/!\[[^\]]*\]\([^)]*\)/g, "") // 图片整个去掉,留着 alt 反而是半句话
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1") // 链接留文字
    .replace(/[*_`~]/g, "") // 强调、代码
    .replace(/\s+/g, " ")
    .trim();
  if (!text) return "";
  return text.length > limit ? `${text.slice(0, limit)}…` : text;
}
