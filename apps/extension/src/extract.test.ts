import { describe, expect, it } from "vitest";
import { buildHtml, buildPayload, escapeAttr, pickSite, META } from "./extract";

const URL_ = "https://news.example.com/post/1";

describe("pickSite", () => {
  it("页面有 og:site_name 就用它", () => {
    expect(pickSite("少数派", "张三", "sspai.com", "fallback")).toBe("少数派");
  });

  it("site 退化成作者名时退回域名", () => {
    // 这是个真踩过的坑:页面缺 og:site_name 时 Defuddle 把作者名填进 site,
    // 直接用就会存出"站点:张三"
    expect(pickSite("张三", "张三", "example.com", "fallback")).toBe("example.com");
  });

  it("site 和域名都没有时用兜底", () => {
    expect(pickSite("", "", "", "example.com")).toBe("example.com");
  });

  it("两边的空白都不算数", () => {
    expect(pickSite("   ", "  ", "   ", "example.com")).toBe("example.com");
  });
});

describe("escapeAttr", () => {
  it("引号和尖括号被转义,免得撑破属性", () => {
    expect(escapeAttr('a"b<c>&d')).toBe("a&quot;b&lt;c&gt;&amp;d");
  });

  it("& 先转义,不会把后面转义出来的实体再转一次", () => {
    // 顺序错了会得到 &amp;quot; 这种双重转义,值就烂了
    expect(escapeAttr("&quot;")).toBe("&amp;quot;");
  });
});

describe("buildHtml", () => {
  it("带上原文地址和全部元数据", () => {
    const html = buildHtml("<p>正文</p>", [[META.title, "标题"]], URL_);
    expect(html).toContain('<meta name="quire-title" content="标题">');
    expect(html).toContain('<meta name="source-url" content="https://news.example.com/post/1">');
    expect(html).toContain("<!--StartFragment-->");
  });

  it("StartFragment / EndFragment 成对", () => {
    // 桌面端按这两个标记定位片段,少一个就切错位置
    const html = buildHtml("<p>正文</p>", [], URL_);
    expect(html.match(/<!--StartFragment-->/g)).toHaveLength(1);
    expect(html.match(/<!--EndFragment-->/g)).toHaveLength(1);
  });
});

describe("buildPayload", () => {
  it("空值不写进载荷", () => {
    // 桌面端收到空字符串还得挨个判空,不如压根别发
    const p = buildPayload(
      { title: "标题", author: "", description: "", published: "", image: "" },
      { content: "<p>正文</p>", markdown: "正文", url: URL_, fallbackTitle: "fb" },
    );
    const keys = p.metas.map(([k]) => k);
    expect(keys).toContain(META.title);
    expect(keys).not.toContain(META.author);
    expect(keys).not.toContain(META.excerpt);
    expect(p.html).not.toContain("quire-author");
  });

  it("必带协议版本,桌面端靠它认这条路", () => {
    const p = buildPayload(
      { title: "标题" },
      { content: "<p>x</p>", markdown: "x", url: URL_, fallbackTitle: "fb" },
    );
    expect(p.metas).toContainEqual([META.version, "1"]);
  });

  it("标题抽不到时用兜底", () => {
    const p = buildPayload(
      { title: "  " },
      { content: "<p>x</p>", markdown: "x", url: URL_, fallbackTitle: "页面标题" },
    );
    expect(p.metas).toContainEqual([META.title, "页面标题"]);
  });

  it("URL 坏了也不会让整次剪藏失败", () => {
    const p = buildPayload(
      { title: "标题" },
      { content: "<p>x</p>", markdown: "x", url: "不是地址", fallbackTitle: "fb" },
    );
    expect(p.metas).toContainEqual([META.title, "标题"]);
  });

  it("元数据值里的引号不会撑破 HTML", () => {
    // 标题里带引号是常事(书名号、英文缩写),不转义就等于让页面结构乱掉,
    // 桌面端那边 parse 出来的字段也就全错了
    const p = buildPayload(
      { title: '他说"很好"' },
      { content: "<p>x</p>", markdown: "x", url: URL_, fallbackTitle: "fb" },
    );
    expect(p.html).toContain("&quot;很好&quot;");
    expect(p.html).not.toContain('content="他说"');
  });
});

/**
 * 抽取前的正文预处理。
 *
 * 两个真实病灶:懒加载图片的 `data-src` Defuddle 不认,转出来就是一张
 * 被整个丢掉的图;`<video>`/`<iframe>` 被 turndown 原样保留成 HTML 标签,
 * 到了 Quire 那边(html:false + DOMPurify 两道防线)只会被转义成一行
 * 字面代码或整个洗掉。都得在交给 Defuddle **之前**处理。
 */
import { prepareDocumentForExtraction } from "./extract";

const LABELS = {
  video: "▶ 视频",
  audio: "▶ 音频",
  embed: "▶ 嵌入内容",
  videoRemote: "▶ 视频(到原文观看)",
  audioRemote: "▶ 音频(到原文观看)",
  embedRemote: "▶ 嵌入内容(到原文观看)",
};

function parse(html: string): Document {
  return new DOMParser().parseFromString(html, "text/html");
}

describe("prepareDocumentForExtraction", () => {
  it("懒加载的 data-src 提升成 src,不然图整个消失", () => {
    const d = parse('<img src="data:image/gif;base64,R0lGOD" data-src="https://cdn.example.com/real.jpg">');
    const out = prepareDocumentForExtraction(d, URL_, LABELS);
    expect(out.querySelector("img")?.getAttribute("src")).toBe("https://cdn.example.com/real.jpg");
  });

  it("data-src 的常见变体也认", () => {
    const d = parse('<img data-original="https://cdn.example.com/o.jpg">');
    const out = prepareDocumentForExtraction(d, URL_, LABELS);
    expect(out.querySelector("img")?.getAttribute("src")).toBe("https://cdn.example.com/o.jpg");
  });

  it("只有 data-srcset 时取分辨率最大的那档", () => {
    const d = parse('<img data-srcset="https://a.example.com/s.jpg 480w, https://a.example.com/l.jpg 1080w">');
    const out = prepareDocumentForExtraction(d, URL_, LABELS);
    expect(out.querySelector("img")?.getAttribute("src")).toBe("https://a.example.com/l.jpg");
  });

  it("相对路径转绝对,不然 Quire 里对着自家协议解析就是裂图", () => {
    const d = parse('<img src="/img/cover.jpg">');
    const out = prepareDocumentForExtraction(d, URL_, LABELS);
    expect(out.querySelector("img")?.getAttribute("src")).toBe("https://news.example.com/img/cover.jpg");
  });

  it("srcset 拆掉,Defuddle 才不会从里面拿相对地址", () => {
    const d = parse('<img src="https://news.example.com/a.jpg" srcset="/b.jpg 2x">');
    const out = prepareDocumentForExtraction(d, URL_, LABELS);
    expect(out.querySelector("img")?.hasAttribute("srcset")).toBe(false);
  });

  it("视频换成可点的链接", () => {
    const d = parse('<video src="/media/clip.mp4" controls></video>');
    const out = prepareDocumentForExtraction(d, URL_, LABELS);
    const a = out.querySelector("a");
    expect(a?.getAttribute("href")).toBe("https://news.example.com/media/clip.mp4");
    expect(a?.textContent).toBe("▶ 视频");
    expect(out.querySelector("video")).toBeNull();
  });

  it("地址写在子 source 里的也接得住", () => {
    const d = parse('<video controls><source src="https://m.example.com/v.mp4" type="video/mp4"></video>');
    const out = prepareDocumentForExtraction(d, URL_, LABELS);
    expect(out.querySelector("a")?.getAttribute("href")).toBe("https://m.example.com/v.mp4");
  });

  it("音频和嵌入一样转链接", () => {
    const d = parse('<audio src="https://m.example.com/a.mp3"></audio><iframe src="/embed/x"></iframe>');
    const out = prepareDocumentForExtraction(d, URL_, LABELS);
    const links = Array.from(out.querySelectorAll("a"));
    expect(links.map((l) => l.getAttribute("href"))).toEqual([
      "https://m.example.com/a.mp3",
      "https://news.example.com/embed/x",
    ]);
  });

  it("YouTube 嵌入也换链接——Defuddle 默认转成图片占位,在 Quire 里必裂", () => {
    const d = parse('<iframe src="https://www.youtube.com/embed/dQw4w9WgXcQ"></iframe>');
    const out = prepareDocumentForExtraction(d, URL_, LABELS);
    expect(out.querySelector("a")?.getAttribute("href")).toBe("https://www.youtube.com/embed/dQw4w9WgXcQ");
    expect(out.querySelector("iframe")).toBeNull();
  });

  it("埋点/统计的 iframe 直接剔除,连链接都不留", () => {
    // 新浪 sbeacon、百度统计、GA 这类跟踪 iframe,收录进正文就是垃圾行
    const d = parse('<iframe src="https://sbeacon.sina.com.cn/ckctl.html"></iframe><article><p>正文</p></article>');
    const out = prepareDocumentForExtraction(d, URL_, LABELS);
    expect(out.querySelectorAll("a").length).toBe(0);
    expect(out.querySelector("iframe")).toBeNull();
  });

  it("没有 src 的 iframe 不剔——它可能是 JS 稍后填充的播放器,链接指向原文", () => {
    // 新浪播放器 iframe 的 src 初始为空:上一版把它当空 iframe 剔了,
    // 视频入口跟着消失。留链接指向原文,顶多对真垃圾多一行,不能丢入口
    const d = parse('<iframe></iframe><article><p>正文</p></article>');
    const out = prepareDocumentForExtraction(d, URL_, LABELS);
    expect(out.querySelector("article a")?.getAttribute("href")).toBe(URL_);
  });

  it("正文容器外的媒体搬进正文开头——不搬,Defuddle 的正文选择会把它整个剔掉", () => {
    // 新浪这类视频新闻页:播放器在正文容器上方,Defuddle 只认正文,
    // 视频转的链接留在原位就等于不存在
    const d = parse('<video src="https://m.example.com/v.mp4"></video><article><p>正文</p></article>');
    const out = prepareDocumentForExtraction(d, URL_, LABELS);
    const a = out.querySelector("article a");
    expect(a?.getAttribute("href")).toBe("https://m.example.com/v.mp4");
    expect(out.querySelector("video")).toBeNull();
  });

  it("没有明确正文容器时不硬塞,链接留在原位", () => {
    const d = parse('<div><p>散着的正文</p></div><video src="https://m.example.com/v.mp4"></video>');
    const out = prepareDocumentForExtraction(d, URL_, LABELS);
    expect(out.querySelector("a")?.getAttribute("href")).toBe("https://m.example.com/v.mp4");
    expect(out.querySelector("article")).toBeNull();
  });

  it("取不到地址的媒体不留死链,链接指向原文", () => {
    const d = parse("<video controls></video>");
    const out = prepareDocumentForExtraction(d, URL_, LABELS);
    expect(out.querySelector("a")?.getAttribute("href")).toBe(URL_);
  });

  it("blob: 流地址不进链接——跨会话即死链,指向原文页", () => {
    // 新浪这类 JS 注入播放器的新闻页,video 的 src 全是 blob:;
    // 存成 blob 链接用户点开是空的,指向原文至少有入口
    const d = parse('<video src="blob:https://video.example.com/abc-123"></video>');
    const out = prepareDocumentForExtraction(d, URL_, LABELS);
    expect(out.querySelector("a")?.getAttribute("href")).toBe(URL_);
  });

  it("正常的图不折腾,只做绝对化", () => {
    const d = parse('<img src="https://news.example.com/ok.jpg" alt="正常">');
    const out = prepareDocumentForExtraction(d, URL_, LABELS);
    const img = out.querySelector("img");
    expect(img?.getAttribute("src")).toBe("https://news.example.com/ok.jpg");
    expect(img?.getAttribute("alt")).toBe("正常");
  });

  it("传进来的文档不被改动——改的是克隆", () => {
    const page = parse('<img data-src="https://cdn.example.com/real.jpg">');
    prepareDocumentForExtraction(page, URL_, LABELS);
    expect(page.querySelector("img")?.getAttribute("src")).toBeNull();
  });
});

/**
 * 预处理得真的接进剪藏流程。函数单测全绿但 content.ts 不调它的话,
 * 用户那边的图片视频照样丢——「各自都对,就是没接上」这种 bug,
 * 只能对着源码看。
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

describe("预处理接进了 content.ts", () => {
  it("Defuddle 之前先做预处理,而且吃的必须是预处理的结果", () => {
    const src = readFileSync(resolve(process.cwd(), "src/content.ts"), "utf8");
    const prepared = src.indexOf("const prepared = prepareDocumentForExtraction(document");
    const parse = src.indexOf("new Defuddle(prepared,");
    expect(prepared, "content.ts 得以 prepared 接住预处理结果").toBeGreaterThan(-1);
    expect(parse, "Defuddle 得吃 prepared,不是别的——传原始文档等于没做").toBeGreaterThan(-1);
    expect(prepared, "预处理必须在 Defuddle 之前跑").toBeLessThan(parse);
  });
});
