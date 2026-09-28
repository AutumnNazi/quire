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
