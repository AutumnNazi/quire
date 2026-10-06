import { describe, expect, it } from "vitest";

import { highlight, plain } from "./highlight";

/** 把节点树拍平成 "文字|文字" 这样,顺便看有没有 mark */
function render(nodes: Node[]): { text: string; marks: string[] } {
  let text = "";
  const marks: string[] = [];
  for (const n of nodes) {
    if (n.nodeType === Node.TEXT_NODE) {
      text += n.textContent ?? "";
    } else if (n.nodeName === "MARK") {
      text += n.textContent ?? "";
      marks.push(n.textContent ?? "");
    }
  }
  return { text, marks };
}

describe("搜索高亮", () => {
  it("没有命中就整段原样返回", () => {
    const out = render(highlight("一段普通文字", []));
    expect(out.text).toBe("一段普通文字");
    expect(out.marks).toEqual([]);
  });

  it("标出来的正好是命中词", () => {
    const out = render(highlight("前面讲苹果后面讲香蕉", [[3, 5]]));
    expect(out.text).toBe("前面讲苹果后面讲香蕉");
    expect(out.marks).toEqual(["苹果"]);
  });

  /** **下标是字符不是字节。** 中文一个字符三个字节,按字节切会切在半个字上,
   *  界面上就是一个乱码方块 —— 而这条用英文样例根本测不出来 */
  it("中文按字符切不切坏字", () => {
    const s = "这是一篇讲数据库优化的文章";
    const chars = Array.from(s);
    const at = chars.indexOf("数");
    const out = render(highlight(s, [[at, at + 2]]));
    expect(out.marks).toEqual(["数据"]);
    expect(out.text).toBe(s);
  });

  /** **标题和摘要都来自用户浏览过的网页**,那里面可能有 `<script>`。
   *  高亮是最容易被人顺手写成 innerHTML 的地方,所以这条专门盯着:
   *  就算后端把整段都标成命中,也不许解析成元素 */
  it("内容里的标签不会被解析成元素", () => {
    const evil = '<img src=x onerror="alert(1)">';
    const nodes = highlight(evil, [[0, evil.length]]);
    // 整段都被标成命中,但它**仍然是一个文本节点**
    expect(nodes).toHaveLength(1);
    expect((nodes[0] as HTMLElement).tagName).toBe("MARK");
    expect((nodes[0] as HTMLElement).querySelector("img")).toBeNull();
    expect(nodes[0].textContent).toBe(evil);
  });

  it("多段命中都标上", () => {
    const chars = Array.from("苹果 香蕉 苹果");
    const second = chars.lastIndexOf("苹");
    const out = render(highlight("苹果 香蕉 苹果", [[0, 2], [second, second + 2]]));
    expect(out.marks).toEqual(["苹果", "苹果"]);
    expect(out.text).toBe("苹果 香蕉 苹果");
  });

  /** 重叠的合并,不然界面上是两段套着的底色、中间一道接缝 */
  it("重叠的命中不产生两段底色", () => {
    const out = render(highlight("数据库优化", [[0, 3], [2, 5]]));
    expect(out.marks).toEqual(["数据库优化"]);
  });

  /** **标错的比不标的更糟。** 越界的区间宁可丢掉,也不能切出乱码 */
  it("越界的区间被丢掉而不是切坏", () => {
    const s = "短文本";
    const out = render(highlight(s, [[0, 999]]));
    expect(out.text).toBe(s);
    expect(out.marks).toEqual([]);
  });

  it("起点大于终点的区间被丢掉", () => {
    const out = render(highlight("短文本", [[3, 1]]));
    expect(out.marks).toEqual([]);
  });

  it("负数区间被丢掉", () => {
    const out = render(highlight("短文本", [[-2, 2]]));
    expect(out.marks).toEqual([]);
  });

  /** 乱序的区间得先排,不然先遇到靠后的那个会把靠前的整段吃掉 */
  it("乱序的区间照样标对", () => {
    const out = render(highlight("甲乙丙丁", [[2, 3], [0, 1]]));
    expect(out.marks).toEqual(["甲", "丙"]);
    expect(out.text).toBe("甲乙丙丁");
  });

  it("空文本不炸", () => {
    expect(render(highlight("", [[0, 1]])).text).toBe("");
  });

  it("plain 把普通摘要包成没有命中的形状", () => {
    expect(plain("摘要")).toEqual({ text: "摘要", marks: [] });
  });

  it("plain 传 null 还是 null", () => {
    expect(plain(null)).toBeNull();
  });
});