import { describe, expect, it } from "vitest";
import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { SHORTCUTS } from "./shortcuts";

/**
 * 快捷键面板**不许撒谎**。
 *
 * 面板本身是一张手写的表(见 `shortcuts.ts` 顶部为什么不做成判定驱动),
 * 而手写的表跟代码分叉是迟早的事。分叉的两个方向都很坏:
 *
 * - **表里有、代码里没有**:用户照着按,没反应。他不会再翻第二遍表,
 *   只会觉得这个软件"有些键不灵"
 * - **代码里有、表里没有**:那个键**等于不存在**——用户不知道能这么用,
 *   就永远用不上,而做它的那半天白花了
 *
 * 所以这一组拿源码里真实的按键判定(而不是再抄一遍)去核那张表。
 */

/** 扫出来的一个按键判定:是哪个键、有没有配修饰键。 */
interface Found {
  key: string;
  mod: boolean;
  where: string;
}

/**
 * 把源码里所有"拿按键做判定"的地方抠出来。
 *
 * 认三种写法:
 * - `e.key === "F5"` / `event.key === "Enter"`
 * - `e.key.toLowerCase() === "v"`(大小写不敏感的字母键)
 * - `key === "j"`(上面 `const key = e.key.toLowerCase()` 那个局部变量)
 *
 * **`e.ctrlKey || e.metaKey` 在同一行或前一行就算"配了修饰键"**,
 * 而不是只看同一行——源码里那个条件常常折行写(见 Ctrl+V 和 Ctrl+A)
 */
function foundKeys(): Found[] {
  const out: Found[] = [];
  const files = readdirSync(resolve(process.cwd(), "src")).filter(
    (f) => f.endsWith(".ts") && !f.endsWith(".test.ts") && f !== "shortcuts.ts" && f !== "i18n.ts",
  );
  const re = /(?:\w+\.)?key(?:\s*\.toLowerCase\(\))?\s*===\s*"([^"]*)"/g;
  for (const file of files) {
    const src = readFileSync(resolve(process.cwd(), "src", file), "utf8");
    const lines = src.split("\n");
    for (const m of src.matchAll(re)) {
      // 往前两行找修饰键:`if ((e.ctrlKey || e.metaKey) && e.key…` 折行很常见
      const lineNo = src.slice(0, m.index).split("\n").length - 1;
      const near = lines.slice(Math.max(0, lineNo - 1), lineNo + 1).join(" ");
      out.push({
        key: m[1].toLowerCase(),
        mod: /ctrlKey|metaKey/.test(near),
        where: `${file}:${lineNo + 1}`,
      });
    }
  }
  return out;
}

const FOUND = foundKeys();

describe("快捷键表", () => {
  it("扫到了判定,不是空跑一趟", () => {
    // **这条是给自己的测量仪表的。** 扫描的正则哪天失配(比如有人把
    // `e.key ===` 换成 `e.code ===`),下面几条会因为"两边都空"而全绿,
    // 而它们守的东西一个都没守
    expect(FOUND.length, "一个按键判定都没扫到,正则多半失配了").toBeGreaterThan(8);
    expect(FOUND.map((f) => f.key)).toContain("f5");
    expect(FOUND.map((f) => f.key)).toContain("j");
  });

  it("表里写的每个键,代码里都真的判了", () => {
    // 表里有、代码里没有 = 骗用户。他照着按没反应,不会怪表,只会怪软件
    const real = new Set(FOUND.map((f) => f.key));
    const lies = SHORTCUTS.flatMap((g) => g.items)
      .flatMap((i) => i.keys)
      .filter((k) => !real.has(k.toLowerCase()));
    expect(lies, "面板上写了这些键,但代码里没人判它们").toEqual([]);
  });

  it("代码里判了的每个键,表上也得有", () => {
    // **反过来漏的后果更隐蔽**:那个键能用,只是没人知道。
    // 用户不会去翻源码,他只会在某天看别人用时惊讶"还有这个?"
    const listed = new Set(
      SHORTCUTS.flatMap((g) => g.items).flatMap((i) => i.keys.map((k) => k.toLowerCase())),
    );
    // 「焦点在某个元素上按回车/空格激活它」是 button 的通用语义,
    // 不是我们发明的快捷键(`detail` 那个标题就是 role="button"),
    // 不值得占面板一行 —— 但要**点名**排除,不是一揽子放过
    const a11y = new Set(["enter", " "]);
    const missing = [
      ...new Set(FOUND.filter((f) => !a11y.has(f.key)).map((f) => f.key)),
    ].filter((k) => !listed.has(k));
    expect(missing, "这些键代码里能用,但面板上没写").toEqual([]);
  });

  it("带修饰键的那几条,代码里真的按了修饰键", () => {
    // 面板上写「Ctrl+A」而代码里只判了 `a` 的话,用户按 A 就会全选——
    // 而这会让他后面敲的每一个 a 都被吞掉
    for (const item of SHORTCUTS.flatMap((g) => g.items).filter((i) => i.mod)) {
      for (const key of item.keys) {
        const hits = FOUND.filter((f) => f.key === key.toLowerCase() && f.mod);
        expect(
          hits.length,
          `面板说「${item.caps.join("+")}」是快捷键,但代码里 ${key} 没配 Ctrl/Cmd`,
        ).toBeGreaterThan(0);
      }
    }
  });

  it("不带修饰键的那几条,代码里也真的不要求修饰键", () => {
    // 反方向:面板写「r 标已读」而代码里判的是 Ctrl+R 的话,
    // 用户光按 r 什么也不会发生
    for (const item of SHORTCUTS.flatMap((g) => g.items).filter((i) => !i.mod)) {
      for (const key of item.keys) {
        const plain = FOUND.filter((f) => f.key === key.toLowerCase() && !f.mod);
        expect(
          plain.length,
          `面板说「${item.caps.join(" ")}」不用修饰键,但代码里 ${key} 只在配 Ctrl/Cmd 时认`,
        ).toBeGreaterThan(0);
      }
    }
  });

  it("每一行的键面和判定一一对应", () => {
    // `keys` 是判定用的(arrowdown),`caps` 是显示用的(↓)。
    // **键面对不上就是漏了一个键的显示**,而漏掉的那个在面板上
    // 会变成一行没有键的说明,用户照着按什么也不会发生
    //
    // 带修饰键的那几行多一个键面(那个「Ctrl」本身就是一段 `<kbd>`,
    // 塞进主键的文字里会变成一个叫 CtrlA 的键)
    for (const item of SHORTCUTS.flatMap((g) => g.items)) {
      const expected = item.keys.length + (item.mod ? 1 : 0);
      expect(
        item.caps.length,
        `${item.label} 有 ${item.keys.length} 个键,该写 ${expected} 个键面,实际写了 ${item.caps.length} 个`,
      ).toBe(expected);
      for (const cap of item.caps) expect(cap.trim()).not.toBe("");
    }
  });

  it("面板上的文案键都在词典里", async () => {
    // 少一条,那一行右边就是一片空白(kat 找不到键会原样返回键名,
    // 而这里连键名都不是——它是拼出来给面板用的)
    const { t } = await import("./i18n");
    for (const group of SHORTCUTS) {
      expect(t(group.title), `${group.title} 没有文案`).not.toBe(group.title);
      for (const item of group.items) {
        expect(t(item.label), `${item.label} 没有文案`).not.toBe(item.label);
      }
    }
  });
});

/**
 * 表对了,面板还得**接得上**。
 *
 * 这几条挡的是"东西都写好了,就是没人能打开":面板函数写得再漂亮,
 * 入口没接上,它对用户来说就是不存在的东西——而运行时不报任何错
 */
describe("快捷键面板接上了没有", () => {
  const mainSrc = readFileSync(resolve(process.cwd(), "src/main.ts"), "utf8");

  it("工具栏上那个提示是个按钮,点得开", () => {
    // 原来它是个 `<span>`,纯装饰。**不认识的键没人会按**,
    // 所以这张表得有个看得见的入口
    expect(mainSrc, "提示得是按钮").toContain('id="btn-help"');
    expect(mainSrc, "按钮得接上打开").toMatch(/helpEl\.addEventListener\("click",[\s\S]{0,120}openShortcuts\(\)/);
  });

  it("按 ? 也能开,而且能再按一次关上", () => {
    const at = mainSrc.indexOf('e.key === "?"');
    expect(at, "得有 ? 那条判定").toBeGreaterThan(-1);
    const branch = mainSrc.slice(at, at + 300);
    expect(branch, "? 得能开").toContain("openShortcuts()");
    expect(branch, "再按一次得能关").toContain("closeShortcuts()");
  });

  it("面板里的内容走那张表,不是另写一遍", () => {
    // 另写一遍就是第二份真相:表面上看两处都对,分叉的时候
    // 用户按面板上的键没反应,而面板还是"最新"的样子
    const fn = mainSrc.slice(
      mainSrc.indexOf("function renderShortcutPanel"),
      mainSrc.indexOf("function openShortcuts"),
    );
    expect(fn.length, "没找到 renderShortcutPanel").toBeGreaterThan(0);
    expect(fn, "得遍历那张表").toContain("for (const group of SHORTCUTS)");
    expect(fn, "得遍历每条").toContain("for (const item of group.items)");
  });

  it("Esc 关得掉", () => {
    // 一个关不掉的面板比没有面板更糟:它挡着正文,用户只能重启软件
    const at = mainSrc.indexOf('e.key === "Escape" && shortcutPanelEl');
    expect(at, "Esc 那条得认这个面板").toBeGreaterThan(-1);
    expect(mainSrc.slice(at, at + 160), "认得就得关").toContain("closeShortcuts()");
  });

  it("按 ? 之前不用先点一下窗口", () => {
    // 挂在 document 上,不是挂在某个容器里。**挂在列表上就完了**:
    // 焦点在搜索框里的时候按 ? 打不开,而用户最想查快捷键的时候
    // 恰恰是"我是不是漏了什么"的时候
    const at = mainSrc.indexOf('e.key === "?"');
    const before = mainSrc.slice(Math.max(0, at - 2000), at);
    expect(before, "? 那条得在 document 级的键盘处理里").toContain(
      'document.addEventListener("keydown"',
    );
  });
});
