import MarkdownIt from "markdown-it";
import DOMPurify from "dompurify";
import Defuddle from "defuddle";
import type { ClipboardCapture } from "./clipboard";

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
      const defuddle = new Defuddle(doc, {
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
