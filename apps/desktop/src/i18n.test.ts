import { describe, expect, it } from "vitest";
import {
  LOCALES,
  applyI18n,
  getLocale,
  isWireError,
  otherLocale,
  resolveLocale,
  setLocale,
  t,
  wireText,
} from "./i18n";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/** 读源码。**不能走 `import.meta.url`**——jsdom 环境会把它换成 http URL,
 * `fileURLToPath` 直接抛错。vitest 的 cwd 就是包根,拼绝对路径最省事。 */
function source(...parts: string[]): string {
  return readFileSync(resolve(process.cwd(), ...parts), "utf8");
}

/** 把词典抠出来给下面几条断言用。词典本身是模块私有的,导出它就得
 *  多一个公开 API 让人能改坏它;测试里读源码反而更直接。 */
function catalog(locale: string): Record<string, string> {
  const src = source("src", "i18n.ts");
  // 切成两本词典。`"zh-CN": {` 和 `en: {` 两种写法都试一遍:
  // 带连字符的语言代码在 TS 里不加引号也能写,但加了也不算错
  const start = [`  "${locale}": {`, `  ${locale}: {`]
    .map((needle) => src.indexOf(needle))
    .filter((i) => i >= 0)
    .sort((a, b) => a - b)[0];
  if (start === undefined) throw new Error(`源码里找不到 ${locale} 那本词典`);
  const end = src.indexOf("\n  },", start);
  const body = src.slice(start, end);

  // 先把跨行的字符串接起来。长文案在源码里折行是常事,按行解析的话
  // 折开的那几条会被整个漏掉——漏掉的键看起来就像"不存在",
  // 报出来的却是"界面引用了词典里没有的键",方向完全反了
  const joined = body.replace(/"\s*\+\s*\n\s*"/g, "");
  const out: Record<string, string> = {};
  const re = /^\s*"([^"]+)":\s*"((?:[^"\\]|\\.)*)",?\s*$/gm;
  for (const m of joined.matchAll(re)) {
    out[m[1]] = m[2].replace(/\\"/g, '"').replace(/\\\\/g, "\\");
  }
  if (Object.keys(out).length === 0) throw new Error(`${locale} 词典一条都没解析出来`);
  return out;
}

const zh = catalog("zh-CN");
const en = catalog("en");

/** 界面上真正用到的键。
 *
 *  **按命名空间抓,不只抓 `t("…")`。** 键也出现在 `showBackendError("…")`
 *  这类地方,只认 `t(` 的话,那些键会被算成"没人用的孤儿",然后下一个人
 *  当成 rot 删掉,一删运行时就在界面上甩出一串英文代号。
 *
 *  错误那批**不从 `main.ts` 扫**——那边写的是模板字符串拼出来的键,
 *  正则扫不到。改从 Rust 源码里抠 `WireError::new("…")`:这样加一条
 *  错误类型却忘了配文案,测试一定红。 */
function usedKeys(): Set<string> {
  const main = source("src", "main.ts");
  const used = new Set<string>();
  // 前缀写全了:少写一个,那个命名空间下的键就整体漏扫。
  // **点号后面不许只收 ASCII**——收窄了的话,有人打错一个中文字,
  // 这个键会被整个漏掉,测试照样绿,而运行时界面上只会甩出那串字
  const key = '"(toolbar|filter|list|clip|trash|detail|toast|confirm|error|watch|batch|lang)\\.[^"]+"';
  // 两头的引号是匹配的一部分,键本身不带
  for (const m of main.matchAll(new RegExp(key, "g"))) used.add(m[0].slice(1, -1));
  for (const m of main.matchAll(/data-i18n(?:-html|-title|-aria|-placeholder)?="([^"]+)"/g)) {
    used.add(m[1]);
  }
  for (const file of ["vault.rs", "lib.rs", "clipboard.rs"]) {
    const rust = source("src-tauri", "src", file);
    // `Self::new` 是 `WireError::internal()` 内部用的同一个构造器,
    // 只认全名的话 `app.internal` 会被当成没人用的键
    for (const m of rust.matchAll(/(?:WireError|Self)::new\("([^"]+)"\)/g)) {
      used.add(`error.${m[1]}`);
    }
  }
  return used;
}

const USED = usedKeys();

describe("词典完整性", () => {
  it("两种语言的键完全一样", () => {
    const missingInEn = Object.keys(zh).filter((k) => !(k in en));
    const missingInZh = Object.keys(en).filter((k) => !(k in zh));
    expect({ missingInEn, missingInZh }).toEqual({ missingInEn: [], missingInZh: [] });
  });

  /** 英文那句少个 `{n}`,界面不报错,只是变成「Marked read as  articles」。
   *  键都对齐了也不代表这句话还是一句通顺的英文。 */
  it("同一条文案的占位符两边一致", () => {
    const placeholders = (s: string) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();
    const mismatched = Object.keys(zh)
      .filter((k) => k in en)
      .filter((k) => placeholders(zh[k]).join(",") !== placeholders(en[k]).join(","))
      .map((k) => `${k}: zh[${placeholders(zh[k])}] en[${placeholders(en[k])}]`);
    expect(mismatched).toEqual([]);
  });

  /** 词典里没人用的键是 rot:改文案时留下的旧键,下一个人会以为
   *  那个功能还在,照着改一遍。 */
  it("没有没人引用的键", () => {
    expect(Object.keys(zh).filter((k) => !USED.has(k))).toEqual([]);
  });

  it("界面里引用的键都存在", () => {
    expect([...USED].filter((k) => !(k in zh))).toEqual([]);
  });

  /** 后端送来的每个错误代号都得有文案。少一条,那个错误在界面上
   *  就只会甩一个 `vault.notFound`。 */
  it("后端每个错误代号都有文案", () => {
    const codes = [...USED].filter((k) => k.startsWith("error."));
    expect(codes.length).toBeGreaterThan(0);
    expect(codes.filter((k) => !(k in zh))).toEqual([]);
  });
});

describe("语言判定", () => {
  it("中文走中文,别的走英文", () => {
    expect(resolveLocale(["zh-CN"])).toBe("zh-CN");
    expect(resolveLocale(["zh"])).toBe("zh-CN");
    expect(resolveLocale(["zh-TW", "en-US"])).toBe("zh-CN");
    expect(resolveLocale(["en-US"])).toBe("en");
    expect(resolveLocale(["de-DE", "fr"])).toBe("en");
  });

  it("首选语言是英文时就用英文,哪怕第二顺位是中文", () => {
    // 有人系统首选英文、第二顺位中文。那是他的选择,别自作主张翻成中文
    expect(resolveLocale(["en-US", "zh-CN"])).toBe("en");
  });

  it("认不出的语言不拦着后面那个", () => {
    // 系统首选德语、第二顺位中文:德语我们没有,那就该给中文,
    // 而不是因为"第一个看不懂"就一路落到英文
    expect(resolveLocale(["de-DE", "zh-CN"])).toBe("zh-CN");
  });

  it("切语言能换回来", () => {
    const before = getLocale();
    const other = otherLocale();
    setLocale(other);
    expect(getLocale()).toBe(other);
    setLocale(before);
    expect(getLocale()).toBe(before);
  });

  it("只认词典里有的语言", () => {
    expect(LOCALES).toEqual(["zh-CN", "en"]);
  });
});

describe("t()", () => {
  it("取当前语言的那本", () => {
    const before = getLocale();
    try {
      setLocale("en");
      expect(t("filter.unread")).toBe("Unread");
      setLocale("zh-CN");
      expect(t("filter.unread")).toBe("未读");
    } finally {
      setLocale(before);
    }
  });

  it("参数替换得进去", () => {
    const before = getLocale();
    try {
      setLocale("zh-CN");
      expect(t("batch.count", { n: 3 })).toBe("已选 3 篇");
      setLocale("en");
      expect(t("batch.count", { n: 3 })).toBe("3 selected");
    } finally {
      setLocale(before);
    }
  });

  /** 缺参数就把 `{n}` 原样留着。悄悄换成空串的话,「导入了  篇」
   *  这种话得盯着屏幕看好几遍才反应过来少了个数字。 */
  it("参数缺了就留下占位符,不留空", () => {
    const before = getLocale();
    try {
      setLocale("zh-CN");
      expect(t("batch.count")).toBe("已选 {n} 篇");
      expect(t("batch.count", { m: 3 })).toBe("已选 {n} 篇");
    } finally {
      setLocale(before);
    }
  });

  it("查不到的键原样返回,不返回空串", () => {
    // 返回空串的话,界面上就是一片空白,用户以为软件坏了
    expect(t("压根不存在的键")).toBe("压根不存在的键");
  });
});

describe("applyI18n", () => {
  it("把标了 data-i18n 的地方填上文案", () => {
    const before = getLocale();
    const host = document.createElement("div");
    try {
      setLocale("en");
      host.innerHTML = `
        <button data-i18n="filter.trash" data-i18n-title="filter.trash.title"></button>
        <input data-i18n-placeholder="toolbar.search.placeholder" />
        <div data-i18n-aria="toolbar.filters.aria"></div>
      `;
      applyI18n(host);

      const button = host.querySelector("button")!;
      expect(button.textContent).toBe("Trash");
      expect(button.title).toBe(
        "Deleted clippings. Restoring puts them back exactly where they were.",
      );
      expect(host.querySelector("input")!.getAttribute("placeholder")).toBe("Search…");
      expect(host.querySelector("div")!.getAttribute("aria-label")).toBe("Filter clippings");
    } finally {
      setLocale(before);
    }
  });

  it("带 kbd 的空态走 innerHTML,其余走 textContent", () => {
    const before = getLocale();
    const host = document.createElement("div");
    try {
      setLocale("en");
      host.innerHTML = `<p data-i18n-html="list.empty.body"></p>`;
      applyI18n(host);
      // <kbd> 得是活的标签,不然快捷键在界面上就没了
      expect(host.querySelectorAll("kbd").length).toBeGreaterThan(0);
    } finally {
      setLocale(before);
    }
  });
});

describe("后端错误", () => {
  it("认出错误对象", () => {
    expect(isWireError({ code: "vault.notFound", args: {} })).toBe(true);
    expect(isWireError(null)).toBe(false);
    expect(isWireError("vault.notFound")).toBe(false);
    // 数组也是 object,只查 typeof 会把它当错误,取出来的 code 是 undefined
    expect(isWireError(["vault.notFound"])).toBe(false);
    expect(isWireError({ args: {} })).toBe(false);
  });

  it("按当前语言翻出后端那句话", () => {
    const before = getLocale();
    try {
      setLocale("zh-CN");
      expect(wireText({ code: "vault.notFound", args: { detail: "a.md" } })).toBe("剪藏不存在:a.md");
      setLocale("en");
      expect(wireText({ code: "vault.notFound", args: { detail: "a.md" } })).toBe(
        "No such clipping: a.md",
      );
    } finally {
      setLocale(before);
    }
  });

  it("没有参数的错误也翻得出来", () => {
    const before = getLocale();
    try {
      setLocale("en");
      expect(wireText({ code: "vault.poisoned" })).toBe(
        "Internal state is broken, please restart Quire",
      );
    } finally {
      setLocale(before);
    }
  });

  it("代号查不到就把代号原样返回", () => {
    // 返回空串的话,界面上就是一片空白,用户只能猜软件是不是坏了
    expect(wireText({ code: "vault.压根没有这个" })).toBe("error.vault.压根没有这个");
  });

  it("不是错误对象时退回一句字符串", () => {
    expect(wireText(new Error("boom"))).toBe("boom");
    expect(wireText("就是一句字符串")).toBe("就是一句字符串");
  });
});
