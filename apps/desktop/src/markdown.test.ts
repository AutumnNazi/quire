import { describe, expect, it } from "vitest";
import { clipToMarkdown, renderMarkdown } from "./markdown";
import type { ClipboardCapture } from "./clipboard";

/** 一段真实的剪贴板 HTML 片段:带导航和页脚,正文在 <article> 里。 */
const HTML_FRAGMENT = `<html><head><title>页面标题</title></head><body>
    <article>
      <h1>深入理解所有权</h1>
      <p>所有权是 Rust 最核心的概念之一。</p>
      <p>第二段正文,用来确认抽取没有只取第一段。</p>
    </article>
    <nav><a href="/">导航</a></nav>
    <footer>页脚</footer>
  </body></html>`;

function capture(over: Partial<ClipboardCapture> = {}): ClipboardCapture {
  return { html: HTML_FRAGMENT, text: "纯文本兜底", url: "https://example.com/p", meta: {}, ...over };
}

describe("clipToMarkdown:裸剪贴板路径", () => {
  it("产出的是 Markdown,不是 HTML", () => {
    // 这条是回归测试。早期从 "defuddle" 主入口导入,那里没接 Markdown 转换,
    // markdown: true 被静默忽略,content 一直是 HTML——类型检查、构建、
    // Rust 测试全都发现不了,存进 .md 的却是 HTML。
    const { markdown } = clipToMarkdown(capture());
    expect(markdown).toContain("所有权是 Rust 最核心的概念之一");
    expect(markdown).not.toContain("<p>");
    expect(markdown).not.toContain("<article>");
  });

  it("抽正文时把导航和页脚剔掉", () => {
    const { markdown } = clipToMarkdown(capture());
    expect(markdown).not.toContain("页脚");
  });

  it("没有 HTML 时降级到纯文本,内容不丢", () => {
    const { markdown, excerpt } = clipToMarkdown(capture({ html: null, text: "就这一行纯文本" }));
    expect(markdown).toBe("就这一行纯文本");
    expect(excerpt).toBe("就这一行纯文本");
  });

  it("HTML 抽不出东西时也不返回空", () => {
    const junk = "<html><body><div></div></body></html>";
    const result = clipToMarkdown(capture({ html: junk, text: "兜底的纯文本" }));
    expect(result.markdown.length).toBeGreaterThan(0);
  });

  it("摘要是正文的第一段,不是标题也不是导航", () => {
    const { excerpt } = clipToMarkdown(capture());
    expect(excerpt).toContain("所有权是 Rust 最核心的概念之一");
    expect(excerpt.length).toBeLessThanOrEqual(121); // 120 字 + 省略号
  });

  it("HTML 实体和链接标记在摘要里被清掉", () => {
    const withLink = capture({
      html: null,
      text: "看这个 [深度解析](https://a.com/x) 和 `代码` 还有 **强调**。",
    });
    const { excerpt } = clipToMarkdown(withLink);
    expect(excerpt).toBe("看这个 深度解析 和 代码 还有 强调。");
  });
});

describe("clipToMarkdown:扩展路径", () => {
  it("带 quire-version 时直接用纯文本里的 Markdown,不再过 Defuddle", () => {
    // 扩展已经对着整页跑过一遍了,再过第二遍只会多一处出错的机会
    const result = clipToMarkdown(
      capture({
        html: "<html><body>这段 HTML 会被忽略</body></html>",
        text: "# 扩展给的标题\n\n扩展给的正文。",
        meta: { "quire-version": "1", "quire-title": "扩展的标题" },
      }),
    );
    expect(result.markdown).toBe("# 扩展给的标题\n\n扩展给的正文。");
    expect(result.title).toBe("扩展的标题");
  });

  it("扩展给的元数据会透出来", () => {
    const result = clipToMarkdown(
      capture({ meta: { "quire-version": "1", "quire-site": "少数派", "quire-author": "张三" } }),
    );
    expect(result.meta["quire-site"]).toBe("少数派");
    expect(result.meta["quire-author"]).toBe("张三");
  });

  it("有版本号但纯文本是空的,退回 Defuddle", () => {
    // 扩展异常时不能因此把内容丢了
    const result = clipToMarkdown(capture({ text: "   ", meta: { "quire-version": "1" } }));
    expect(result.markdown).toContain("所有权是 Rust 最核心的概念之一");
  });
});

describe("renderMarkdown:渲染前必须消毒", () => {
  /** 把输出当成真 DOM 解析,而不是拿字符串找。
   *  "不包含 onclick" 这种断言会被转义后的文本骗过去——文本里有 onclick
   *  三个字,和 DOM 里有个 onclick 属性完全是两码事。 */
  function parse(html: string): Document {
    return new DOMParser().parseFromString(html, "text/html");
  }

  it("裸 HTML 里的 script 不会变成真标签", () => {
    const doc = parse(renderMarkdown("正文<script>alert(1)</script>"));
    expect(doc.querySelectorAll("script")).toHaveLength(0);
    // 内容还在,只是变成了普通文本——不能连内容一起吃掉
    expect(doc.body.textContent).toContain("alert(1)");
  });

  it("裸 HTML 上的事件属性不会变成真属性", () => {
    const doc = parse(renderMarkdown('<a href="https://a.com" onclick="alert(1)">点我</a>'));
    // 遍历所有元素,只要有一个带着 on* 属性就是漏了
    const withHandler = [...doc.querySelectorAll("*")].filter((el) =>
      [...el.attributes].some((a) => a.name.startsWith("on")),
    );
    expect(withHandler).toHaveLength(0);
  });

  it("外链一律新标签打开并断掉 opener", () => {
    const doc = parse(renderMarkdown("[链接](https://example.com)"));
    const link = doc.querySelector("a")!;
    expect(link.getAttribute("target")).toBe("_blank");
    const rel = link.getAttribute("rel") ?? "";
    expect(rel).toContain("noopener");
    expect(rel).toContain("noreferrer");
  });

  it("javascript: 链接不会变成可点的 href", () => {
    const doc = parse(renderMarkdown("[点我](javascript:alert(1))"));
    // 断言的是"没有任何 href 是 javascript:",而不是"输出里没有这串字符"——
    // markdown-it 拒了这个链接,剩下的只是转义后的纯文本,那是安全的
    const hrefs = [...doc.querySelectorAll("a")].map((a) => a.getAttribute("href") ?? "");
    expect(hrefs.some((h) => h.toLowerCase().startsWith("javascript:"))).toBe(false);
  });
});
