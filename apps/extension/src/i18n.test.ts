import { describe, expect, it } from "vitest";
import { readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";

/**
 * 扩展文案的完整性。
 *
 * 词典这套东西只有一条价值:**加了新句子,别忘了把它翻出来**。少了这个
 * 测试,漏翻的表现是"英文界面里有一句中文",而这在功能测试里全绿——
 * 语言这件事天生测不出来,只能靠把目录结构本身当成被测对象。
 *
 * 盯五件事:
 *   1. 两份词典的键必须一模一样(漏一份 = 那个语言下显示键名)
 *   2. 代码里用到的每个键都必须在词典里(加句子忘了加键 = 显示不出来)
 *   3. manifest 里的每个 `__MSG_*__` 都必须解析得到
 *   4. `default_locale` 得指向真存在的目录——少了它 Chrome 直接拒绝加载
 *      扩展,而这个错在构建期看不出来
 *   5. 占位符:句子里用到的 `$name$` 都得在 `placeholders` 里声明
 *
 * 词典**按目录扫出来**,不是写死两个文件名:写死的话,新建一个语言目录
 * 却忘了往测试里登记,它就静默地不参与任何检查。
 */

interface Message {
  message: string;
  description?: string;
  placeholders?: Record<string, { content: string }>;
}

const read = (p: string): string => readFileSync(resolve(process.cwd(), p), "utf8");
const json = <T>(p: string): T => JSON.parse(read(p)) as T;

const LOCALES_DIR = "public/_locales";
const locales = readdirSync(resolve(process.cwd(), LOCALES_DIR), { withFileTypes: true })
  .filter((d) => d.isDirectory())
  .map((d) => d.name);

/** locale 目录名 -> 词典。中文那份是 default_locale,拿它当基准。 */
const catalogs = Object.fromEntries(
  locales.map((l) => [l, json<Record<string, Message>>(`${LOCALES_DIR}/${l}/messages.json`)]),
) as Record<string, Record<string, Message>>;

const DEFAULT_LOCALE = json<{ default_locale?: string }>("public/manifest.json").default_locale;
const base = catalogs[DEFAULT_LOCALE ?? ""] ?? {};
const manifestRaw = read("public/manifest.json");
const backgroundJs = read("public/background.js");
const contentTs = read("src/content.ts");

describe("扩展文案", () => {
  it("至少要有一份词典,还得是两份", () => {
    // 一份 = 没得选语言,那这套机制白搭。少于两份则说明有语言漏建了
    expect(locales.length, `没扫到词典目录,或只有一个: ${locales.join(",")}`).toBe(2);
    expect(locales).toContain(DEFAULT_LOCALE);
  });

  it("每份词典的键都一模一样", () => {
    // 漏一份的话,那个语言下 `chrome.i18n.getMessage` 返回空串,
    // 界面上就是一句空白,比中英文混排还难查
    const expected = Object.keys(base).sort();
    for (const locale of locales) {
      expect(Object.keys(catalogs[locale]).sort(), `${locale} 的键对不上`).toEqual(expected);
    }
  });

  it("没有空句子", () => {
    for (const locale of locales) {
      for (const [key, entry] of Object.entries(catalogs[locale])) {
        expect(entry.message.trim(), `${locale}/${key} 不能是空的`).not.toBe("");
      }
    }
  });

  it("英文词典里不能混进中文", () => {
    // 中文界面写英文还能看懂;反过来是"这个软件是不是坏了"
    for (const [key, entry] of Object.entries(catalogs.en ?? {})) {
      expect(/[一-鿿]/.test(entry.message), `en/${key} 混进了中文: ${entry.message}`).toBe(false);
    }
  });

  it("代码里用到的每个键都在词典里", () => {
    const used = new Set<string>();
    for (const src of [contentTs, backgroundJs]) {
      for (const m of src.matchAll(/\bt\(\s*"([a-z0-9_]+)"/gi)) used.add(m[1]);
    }
    // 五处引用:content.ts 两句,background.js 四个调用点
    expect(used.size).toBeGreaterThanOrEqual(5);
    const missing = [...used].filter((k) => !(k in base) || !(k in (catalogs.en ?? {})));
    expect(missing, "这些键代码在用,词典里没有").toEqual([]);
  });

  it("manifest 里的 __MSG_*__ 都解析得到", () => {
    const refs = [...manifestRaw.matchAll(/__MSG_([a-z0-9_]+)__/gi)].map((m) => m[1]);
    expect(refs.length, "manifest 应该至少引用扩展名、描述和按钮提示").toBeGreaterThanOrEqual(3);
    for (const key of refs) {
      expect(base, `manifest 引用了词典里没有的 ${key}`).toHaveProperty(key);
    }
  });

  it("default_locale 指向真存在的目录", () => {
    // 少了它 Chrome 直接拒绝加载扩展,而这个错在构建期看不出来
    expect(DEFAULT_LOCALE, "manifest 少了 default_locale").toBeTruthy();
    expect(locales, `default_locale ${DEFAULT_LOCALE} 没有对应目录`).toContain(DEFAULT_LOCALE);
  });

  it("词典里的占位符都声明过了", () => {
    // `toast_saved` 传一个标题,词典就只声明一个占位符。
    // 没声明的话,句子里会原样露出 `$title$`
    for (const locale of locales) {
      for (const [key, entry] of Object.entries(catalogs[locale])) {
        const declared = Object.keys(entry.placeholders ?? {});
        const used = new Set([...entry.message.matchAll(/\$([A-Za-z0-9_]+)\$/g)].map((m) => m[1]));
        for (const name of used) {
          expect(declared, `${locale}/${key} 用了 $${name}$ 却没有声明它`).toContain(name);
        }
      }
    }
  });

  it("background.js 的 t() 与 src/i18n.ts 是同一个约定", () => {
    // background.js 是不打包的裸脚本,没法 import,所以那份 t() 是手抄的。
    // 手抄的东西会漂移,这条测试就是拿来发现漂移的
    expect(backgroundJs).toContain("chrome.i18n.getMessage");
    expect(read("src/i18n.ts")).toContain("chrome.i18n.getMessage");
  });

  /**
   * 用户能看见的句子里不该有中文。
   *
   * **上面那条"键都在词典里"抓不到这个。** 把 `t("error_no_content")`
   * 改回一句写死的中文,键还是那个键,两份词典照样一模一样,所有检查全绿——
   * 而结果就是英文用户的图标提示变成一句中文。这就是上一版测试的洞。
   *
   * 所以这里反过来查:**代码里根本不该出现带中文的字符串字面量**。
   * 注释里全是中文,那是给开发者看的,不用翻,所以先剥掉注释再查。
   *
   * 剥法是自己写的扫描器而不是正则:正则分不清 `"a//b"` 里的 `//` 和注释
   * 的开头。**它不认正则字面量**——真出现一个带中文的正则会被误报,
   * 那时候把这里改成认它就行,比现在就写个更复杂的解析器划算。
   *
   * **它还有一个已知的坑:块注释里不能有"星号紧跟斜杠"那对字符。**
   * 注释里随手写一句词典的通配路径(中间带星号那种)就会把注释提前截断,
   * 后面整段当代码解析——本文件前两版都栽在这,头一次是写文档,第二次
   * 是把那句文档原样抄了回来。注释里别写通配写法,background.js 顶上
   * 也记着这一条。
   */
  it("给用户看的句子不许写死中文", () => {
    for (const [name, src] of [
      ["src/content.ts", contentTs],
      ["public/background.js", backgroundJs],
    ] as const) {
      const cjk = stringLiterals(src).filter((s) => /[一-鿿]/.test(s));
      expect(cjk, `${name} 里这几句写死了中文,该走 t()`).toEqual([]);
    }
  });
});

/** 抠出代码里的字符串字面量,跳过注释。 */
function stringLiterals(src: string): string[] {
  const out: string[] = [];
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (c === "/" && src[i + 1] === "/") {
      while (i < src.length && src[i] !== "\n") i++;
      continue;
    }
    if (c === "/" && src[i + 1] === "*") {
      i += 2;
      while (i < src.length && !(src[i] === "*" && src[i + 1] === "/")) i++;
      i += 2;
      continue;
    }
    if (c === '"' || c === "'" || c === "`") {
      const quote = c;
      i++;
      let s = "";
      while (i < src.length && src[i] !== quote) {
        if (src[i] === "\\") {
          s += src[i + 1] ?? "";
          i += 2;
        } else {
          s += src[i];
          i++;
        }
      }
      i++;
      out.push(s);
      continue;
    }
    i++;
  }
  return out;
}

