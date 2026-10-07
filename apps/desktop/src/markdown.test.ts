import { describe, expect, it } from "vitest";
import { clipToMarkdown, renderMarkdown, resolveAssetImages } from "./markdown";
import { t } from "./i18n";
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

/**
 * 媒体与懒加载:剪藏内容里的图片和视频,「没有链接」的两个根子。
 *
 * Defuddle 自己会认 data-src、data-srcset 并把相对地址绝对化(探针实测),
 * 但 data-original / data-lazy-src 这些变体它不认——老 WordPress 站和部分
 * 图床用的就是它们,整张图直接丢。<video>/<iframe> 则被它原样保留成 HTML,
 * 到渲染侧 html:false + DOMPurify 两道防线(刻意不拆,剪藏是不可信输入)
 * 只会被洗成一行字面代码。都得在交给 Defuddle **之前**处理。
 */
describe("clipToMarkdown:媒体与懒加载", () => {
  it("data-original 懒加载变体提升成真地址,而不是整张丢掉", () => {
    const { markdown } = clipToMarkdown(
      capture({
        html: `<html><body><article><p>正文</p><img data-original="https://cdn.example.com/real.jpg" alt="封面"></article></body></html>`,
      }),
    );
    expect(markdown).toContain("![封面](https://cdn.example.com/real.jpg)");
  });

  it("埋点/统计的 iframe 直接剔除,连链接都不留", () => {
    const { markdown } = clipToMarkdown(
      capture({
        html: `<html><body><iframe src="https://sbeacon.sina.com.cn/ckctl.html"></iframe><article><p>正文</p></article></body></html>`,
      }),
    );
    expect(markdown).not.toContain("](https");
    expect(markdown).not.toContain("iframe");
  });

  it("正文容器外的视频搬进正文——不搬会被 Defuddle 的正文选择剔掉", () => {
    // 新浪这类视频新闻页:播放器在正文容器上方。转成链接后若留在原位,
    // Defuddle 只认正文,链接等于不存在
    const { markdown } = clipToMarkdown(
      capture({
        html: `<html><body><video src="https://m.example.com/clip.mp4"></video><article><p>正文</p></article></body></html>`,
      }),
    );
    expect(markdown).toContain("](https://m.example.com/clip.mp4)");
  });

  it("视频变成一条可点的链接,而不是被洗掉", () => {
    const { markdown } = clipToMarkdown(
      capture({
        html: `<html><body><article><p>正文</p><video src="https://m.example.com/clip.mp4" controls></video></article></body></html>`,
      }),
    );
    expect(markdown).toContain("](https://m.example.com/clip.mp4)");
    expect(markdown).not.toContain("<video");
  });

  it("blob: 流地址的视频链接指向原文页,文案如实标注到原文观看", () => {
    // 新浪这类新闻页的播放器是 JS 注入的,src 全是 blob:,
    // blob 只在原页面会话里有效,存下来就是死链;文案也不能骗人
    const { markdown } = clipToMarkdown(
      capture({
        html: `<html><body><article><p>正文</p><video src="blob:https://video.example.com/abc-123"></video></article></body></html>`,
      }),
    );
    // 文案不绑死语言:断言用与实现同源的 i18n 取值
    expect(markdown).toContain(`${t("clip.mediaVideoRemote")}](https://example.com/p)`);
    expect(markdown).not.toContain("blob:");
  });
});

/**
 * 本地化图片的渲染:localize 把远程图下到 md 旁边的 assets 目录,正文里
 * 的地址重写成 `assets/<id>/0.png` 相对路径,基准是 **md 所在目录**——
 * 任何 md 阅读器照这个基准都能显示。WebView 拿相对路径去请求自己的
 * 页面地址,必然 404 裂图——得按同一基准拼绝对路径再转 asset 地址。
 */
describe("resolveAssetImages", () => {
  const toAssetUrl = (p: string) => `asset://test/${p}`;
  const WIN_ROOT = "D:" + String.fromCharCode(92) + "Quire";

  it("assets/ 开头的相对地址拼上库根转成 asset 地址", () => {
    const root = document.createElement("div");
    root.innerHTML = '<img src="assets/m1/0.png" alt="a"><p>x</p>';
    resolveAssetImages(root, WIN_ROOT, toAssetUrl);
    const img = root.querySelector("img");
    expect(img?.getAttribute("src")).toBe(`asset://test/${WIN_ROOT}/assets/m1/0.png`);
  });

  it("库根结尾的分隔符不叠两个", () => {
    const root = document.createElement("div");
    root.innerHTML = '<img src="assets/m1/0.png">';
    resolveAssetImages(root, WIN_ROOT + String.fromCharCode(92), toAssetUrl);
    expect(root.querySelector("img")?.getAttribute("src")).toBe(
      `asset://test/${WIN_ROOT}/assets/m1/0.png`,
    );
  });

  it("远程图和 data: 图不是本地化的产物,原样不动", () => {
    const root = document.createElement("div");
    root.innerHTML =
      '<img src="https://cdn.example.com/a.jpg"><img src="data:image/png;base64,AAA"><img src="assets/m1/1.jpg">';
    resolveAssetImages(root, WIN_ROOT, toAssetUrl);
    const srcs = Array.from(root.querySelectorAll("img")).map((i) => i.getAttribute("src"));
    expect(srcs).toEqual([
      "https://cdn.example.com/a.jpg",
      "data:image/png;base64,AAA",
      `asset://test/${WIN_ROOT}/assets/m1/1.jpg`,
    ]);
  });

  it("库路径还没就绪时不转换,别把地址改坏", () => {
    const root = document.createElement("div");
    root.innerHTML = '<img src="assets/m1/0.png">';
    resolveAssetImages(root, "", toAssetUrl);
    expect(root.querySelector("img")?.getAttribute("src")).toBe("assets/m1/0.png");
  });
});
