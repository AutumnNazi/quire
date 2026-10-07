import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * 冲突守卫接线验得对不对。
 *
 * `main.ts` 顶层就往 document 上挂东西,没法在单测里 import 进来,
 * 所以照 `refresh.test.ts` / `css.test.ts` 的路子读源码验结构。
 *
 * **这些断言挡的回归是"每个人各自都觉得删掉没事"的那种:**
 * 后端查了冲突、前端传了指纹、后端拦下来了写——三段都在,但中间
 * 哪一段没接上,运行时看着完全正常:用户照常用,只在真冲突那天才发现
 * 数据被吃掉了。而那种情况要等用户碰上才暴露,那时候已经晚了
 */
/**
 * 读源码并**把换行统一成 `\n`**。
 *
 * 仓库没有 `.gitattributes`,Windows 上 git 默认 `core.autocrlf=true`:
 * 全新检出的工作副本是 CRLF,而本地编辑器写出来的那份是 LF。跨行锚点
 * 写的是 `\n`,于是同一份代码本地绿、CI 红——而这条测试要验的是接线,
 * 不是换行风格,拿 `\n` 当锚点等于在断言它并不想断言的东西
 */
const readSrc = (rel: string): string =>
  readFileSync(resolve(process.cwd(), rel), "utf8").replace(/\r\n/g, "\n");

describe("冲突守卫接上了没有", () => {
  const mainSrc = readSrc("src/main.ts");
  const apiSrc = readSrc("src/api.ts");
  /** 掐出某一段,免得"全文里有"被别处的同名调用蒙过去 */
  const section = (anchor: string, len: number): string =>
    mainSrc.slice(mainSrc.indexOf(anchor), mainSrc.indexOf(anchor) + len);

  it("api 层的三个写入都把指纹传出去了", () => {
    // 少一个就等于那一处的守卫是空转的。批量路径(批量打标签、批量标记)
    // 不在列 —— 那里没有编辑上下文,给不出指纹
    for (const [name, cmd] of [
      ["setClipTitle", "set_clip_title"],
      ["setClipNote", "set_clip_note"],
      ["setClipBody", "set_clip_body"],
    ]) {
      const at = apiSrc.indexOf(`${name}:`);
      expect(at, `api 里得有 ${name}`).toBeGreaterThan(-1);
      const line = apiSrc.slice(at, at + 200);
      expect(line, `${name} 得把 stamp 传给 ${cmd}`).toContain("stamp: stamp ?? null");
    }
  });

  it("三处写入都在调用时带上了当前指纹", () => {
    // **判据是"读详情时记下的那一份",不是随手 new 一个**。
    // 三处都经 `write(stamp)` 转发,所以要验的是**调用方有没有把
    // activeStamp 交给 write** —— 漏一处,那一处就是个不带指纹的空转守卫,
    // 后端那句 check_stamp 永远走 `None` 那条直接放行
    for (const call of ["write(activeStamp", "await write(activeStamp ?? undefined)"]) {
      const hits = mainSrc.split(call).length - 1;
      expect(hits, `得有三处把 activeStamp 交给 write,找的是 ${call}`).toBe(3);
    }
  });

  it("write 那层原样把指纹传下去,没有中途丢掉", () => {
    // `write` 里漏传 stamp 的话,上面那条照样绿 —— 参数在那儿,只是没往下送
    for (const fn of ["setClipTitle(filename, title, stamp)",
                      "setClipNote(filename, note, stamp)",
                      "setClipBody(clip.filename, markdown, stamp)"]) {
      expect(mainSrc, `${fn} 得原样转发`).toContain(fn);
    }
  });

  it("写成功了会换新的指纹", () => {
    // **写完不换,自己跟自己冲突。** 用户改一次标题之后,下一次改批注
    // 就会撞上一个由他自己制造的"被别处改过"——第二次保存起就再也存不进去
    //
    // **逐处点名,不数总数。** 数总数的话,四处里干掉一处,还剩三处,
    // 数量照样够,断言照样绿——而少记的那一处正是在改完标志之后
    // 继续写批注的那条路,是用户最容易碰上的
    const spots = [
      ["打开详情", "async function openDetail(filename: string)"],
      ["冲突后重读", "async function openDetailNow"],
      ["改完标志刷新", "async function refreshOpenDetail"],
      ["换语言重画", "activeStamp = clip.stamp ?? null;\n        renderDetail(clip);"],
    ];
    // 前几处是 `clip.stamp`,后面两处直接取
    const clipSpots = mainSrc.split("activeStamp = clip.stamp ?? null;").length - 1;
    const inlineSpots =
      mainSrc.split("activeStamp = (await api.readClip(filename)).stamp ?? null;").length - 1;
    expect(clipSpots, "打开/重读/刷新/换语言这四处都得记指纹").toBe(4);
    expect(inlineSpots, "改完标题和存完批注都得重读指纹").toBe(2);
    for (const [what, anchor] of spots) {
      expect(mainSrc, `${what} 那处得还在`).toContain(anchor);
    }
  });

  it("冲突时给的是两个选择,不是一个默认动作", () => {
    const at = mainSrc.indexOf("function reportConflict");
    expect(at, "得有 reportConflict").toBeGreaterThan(-1);
    const fn = section("function reportConflict", 900);
    expect(fn, "得给『读最新的』").toContain("toast.conflict.discard");
    expect(fn, "得给『用我这份覆盖回去』").toContain("toast.conflict.overwrite");
  });

  it("覆盖重试不带指纹", () => {
    // **带了就是把几秒前那份盖回去**,而那正是这个守卫要拦的事。
    // 冲突只能由用户显式解除,不能自动解除
    const fn = section("function reportConflict", 900);
    const overwrite = fn.slice(fn.indexOf("toast.conflict.overwrite"));
    expect(overwrite, "覆盖重试得调不带指纹的那条路").toMatch(
      /retry\(\)|write\(undefined\)/,
    );
  });

  it("冲突时不碰批注输入框", () => {
    // **用户半句话还在里头。** 冲突处理要是重画详情页,那半句就没了,
    // 而它正是最该留住的东西——只有用户自己点『读最新的』才可以重画
    const at = mainSrc.indexOf("attachNoteEditor(area");
    expect(at, "得有批注编辑器").toBeGreaterThan(-1);
    const block = section("attachNoteEditor(area", 1400);
    expect(block, "冲突那一支得先判 isConflict").toContain("isConflict(err)");
    expect(block, "冲突那一支得先判再动别的").toMatch(
      /if\s*\(!isConflict\(err\)\)\s*throw err;\s*reportConflict/,
    );
  });

  it("冲突不是靠一句报错打发的,得真停下来问", () => {
    // 只有红条没有选择的话,用户只能干瞪眼:他既不知道磁盘上那份变成
    // 了什么,也不知道自己刚敲的字还能不能救
    expect(mainSrc, "冲突得弹提示").toContain("showToast(t(\"toast.conflict\"");
  });
});