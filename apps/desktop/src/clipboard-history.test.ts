import { beforeEach, describe, expect, it } from "vitest";

import { all, clear, markSaved, pending, record } from "./clipboard-history";

function cap(
  text: string,
  at: number,
  extra: { html?: string; url?: string; meta?: Record<string, string> } = {},
) {
  return {
    text,
    html: extra.html ?? null,
    url: extra.url ?? null,
    meta: extra.meta,
    at,
  };
}

describe("剪藏历史", () => {
  beforeEach(() => clear());

  it("弹过一次就记下来", () => {
    record(cap("甲", 1));
    expect(pending()).toHaveLength(1);
    expect(pending()[0].text).toBe("甲");
  });

  /** **同一段内容再弹一次要并进已有那条。** 新开一条的话,用户复制同一个
   *  地址十次,抽屉里就是十行一模一样的条目——那还不如没有抽屉 */
  it("同一段内容反复弹出并成一条", () => {
    record(cap("甲", 1));
    record(cap("甲", 2));
    record(cap("甲", 3));
    const p = pending();
    expect(p).toHaveLength(1);
    expect(p[0].seen).toBe(3);
  });

  /** 扩展剪藏时可能第一次只带纯文本、第二次才把 HTML 注入完整。
   *  换上去之后用户补存的那一下才不会丢字段 */
  it("后一次带来的 HTML 换到已有那条上", () => {
    record(cap("甲", 1));
    record(cap("甲", 2, { html: "<b>完整的</b>", url: "https://a.com" }));
    expect(pending()[0].html).toBe("<b>完整的</b>");
    expect(pending()[0].url).toBe("https://a.com");
  });

  /** 扩展偶尔先塞纯文本、后补 HTML。**后来的空 meta 不能把先前那份好的抹掉**——
   *  少了这个判断,用户从历史补存的一篇会丢掉标题和封面,而界面上什么提示都没有 */
  it("后来的空元数据不抹掉先前那份", () => {
    record(cap("甲", 1, { meta: { "quire-title": "完整的标题" } }));
    record(cap("甲", 2));
    expect(pending()[0].meta["quire-title"]).toBe("完整的标题");
  });

  it("后来的完整元数据换上去", () => {
    record(cap("甲", 1));
    record(cap("甲", 2, { meta: { "quire-title": "后来的标题" } }));
    expect(pending()[0].meta["quire-title"]).toBe("后来的标题");
  });

  it("存下来之后从待存里去掉", () => {
    const item = record(cap("甲", 1));
    markSaved(item.id);
    expect(pending()).toHaveLength(0);
    // 但**不消失**,标上已存:用户存完又想看看刚才那批还漏了没
    expect(all()[0].saved).toBe(true);
  });

  /** **内存里的东西不能无限涨。** 用户开着软件一整天,复制几百次是常事 */
  it("超过上限就丢最老的", () => {
    for (let i = 0; i < 260; i += 1) record(cap(`第${i}条`, i));
    const all_ = all();
    expect(all_.length).toBeLessThanOrEqual(200);
    // 最老的那批该没了,最新的还在
    expect(all_[0].text).toBe("第259条");
  });

  it("清空之后什么都不剩", () => {
    record(cap("甲", 1));
    clear();
    expect(all()).toHaveLength(0);
  });

  it("最新弹的在最前面", () => {
    record(cap("旧的", 1));
    record(cap("新的", 2));
    expect(pending()[0].text).toBe("新的");
  });

  it("标一个不存在的 id 不炸", () => {
    expect(() => markSaved("压根没有")).not.toThrow();
  });

  /** 剪贴板里只有一个空格时也会弹。**照样记下来**——那是用户真的复制了
   *  什么,哪怕他没想到。抽屉里摆一行空白看着别扭,但总比"用户明明复制了
   *  东西、Quire 却当没发生过"要好 */
  it("空白内容也记下来,因为那是用户真的复制了", () => {
    record(cap("", 1));
    record(cap("   \n  ", 2));
    expect(pending()).toHaveLength(2);
  });
});