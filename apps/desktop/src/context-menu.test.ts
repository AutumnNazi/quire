import { beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { closeContextMenu, openContextMenuAt } from "./context-menu";

/**
 * 右键菜单的行为。**真渲染真点**,不读源码——这个模块自管生命周期,
 * 最容易坏的就是「开了关不掉」「关了监听还在」,那些只有真的开一轮、
 * 关一轮才看得见。
 */
describe("右键菜单", () => {
  beforeEach(() => {
    closeContextMenu();
  });

  it("打开后挂在 body 上,一个项一个按钮", () => {
    openContextMenuAt(10, 10, [
      { label: "标已读", onClick: () => {} },
      { label: "删除", danger: true, onClick: () => {} },
    ]);
    const menu = document.querySelector(".context-menu");
    expect(menu, "菜单得挂出来").not.toBeNull();
    expect(menu!.querySelectorAll("[role=menuitem]")).toHaveLength(2);
  });

  it("点一项:先关菜单,再执行那个动作", () => {
    const action = vi.fn();
    openContextMenuAt(10, 10, [{ label: "标已读", onClick: action }]);
    (document.querySelector(".context-menu-item") as HTMLButtonElement).click();
    expect(action).toHaveBeenCalledTimes(1);
    // 菜单得先走:动作里再弹面板的话,旧菜单压在面板上就错了
    expect(document.querySelector(".context-menu")).toBeNull();
  });

  it("Esc 关得掉,而且摘了全局监听", () => {
    openContextMenuAt(10, 10, [{ label: "标已读", onClick: () => {} }]);
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(document.querySelector(".context-menu")).toBeNull();
    // 关过之后再按 Esc 不该报错、也不该有残留监听炸出来
    expect(() =>
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })),
    ).not.toThrow();
  });

  it("点菜单外面关,点菜单里面不关", () => {
    openContextMenuAt(10, 10, [{ label: "标已读", onClick: () => {} }]);
    document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(document.querySelector(".context-menu")).toBeNull();

    openContextMenuAt(10, 10, [{ label: "标已读", onClick: () => {} }]);
    (document.querySelector(".context-menu") as HTMLElement).dispatchEvent(
      new MouseEvent("mousedown", { bubbles: true }),
    );
    expect(document.querySelector(".context-menu")).not.toBeNull();
  });

  it("再开一个时,旧的先关——屏上永远只有一个", () => {
    openContextMenuAt(10, 10, [{ label: "甲", onClick: () => {} }]);
    openContextMenuAt(20, 20, [{ label: "乙", onClick: () => {} }]);
    expect(document.querySelectorAll(".context-menu")).toHaveLength(1);
    expect(document.querySelector(".context-menu")!.textContent).toBe("乙");
  });

  it("空菜单不开", () => {
    openContextMenuAt(10, 10, []);
    expect(document.querySelector(".context-menu")).toBeNull();
  });

  it("靠屏幕右缘/下缘的菜单会收进来,不伸出屏幕外", () => {
    // jsdom 的 getBoundingClientRect 全是 0,量出来的"菜单宽高"是 0——
    // 防溢出退化成 min(x, innerWidth-8)。位置靠右时左边距必须被压回去
    openContextMenuAt(2000, 2000, [{ label: "标已读", onClick: () => {} }]);
    const menu = document.querySelector(".context-menu") as HTMLElement;
    expect(Number.parseFloat(menu.style.left)).toBeLessThan(window.innerWidth);
    expect(Number.parseFloat(menu.style.top)).toBeLessThan(window.innerHeight);
  });

  it("danger 项带红字样式", () => {
    openContextMenuAt(10, 10, [
      { label: "标已读", onClick: () => {} },
      { label: "删除", danger: true, onClick: () => {} },
    ]);
    const items = [...document.querySelectorAll(".context-menu-item")];
    expect(items[0].className).not.toContain("danger");
    expect(items[1].className).toContain("danger");
  });
});

describe("右键菜单接上了没有", () => {
  const mainSrc = readFileSync(resolve(process.cwd(), "src/main.ts"), "utf8");
  const itemSrc = readFileSync(resolve(process.cwd(), "src/clip-item.ts"), "utf8");

  /** 取一整个函数体。括号配对找边界,理由同 settings-wiring 里那个 */
  const body = (src: string, anchor: string): string => {
    const at = src.indexOf(anchor);
    if (at < 0) return "";
    let depth = 0;
    let paramsClosed = -1;
    for (let i = at; i < src.length; i += 1) {
      if (src[i] === "(") depth += 1;
      else if (src[i] === ")") {
        depth -= 1;
        if (depth === 0) {
          paramsClosed = i;
          break;
        }
      }
    }
    if (paramsClosed < 0) return "";
    const open = src.indexOf("{", paramsClosed);
    depth = 0;
    for (let i = open; i < src.length; i += 1) {
      if (src[i] === "{") depth += 1;
      else if (src[i] === "}") {
        depth -= 1;
        if (depth === 0) return src.slice(at, i + 1);
      }
    }
    return src.slice(at);
  };

  it("列表项挂了 contextmenu,交出去的是事件和这一篇", () => {
    expect(itemSrc, "列表项得监听右键").toContain(
      'addEventListener("contextmenu", (e) => ctx.onContextMenu(e, clip))',
    );
    expect(itemSrc, "上下文接口得有这个回调").toContain(
      "onContextMenu: (event: MouseEvent, clip: ClipSummary) => void;",
    );
  });

  it("main.ts 接住了回调,菜单跟着这一篇的状态变", () => {
    const fn = body(mainSrc, "function openClipContextMenu(event: MouseEvent, clip: ClipSummary)");
    expect(fn.length, "没找到菜单构建函数").toBeGreaterThan(0);
    // 已读/未读菜单项跟着状态走——固定文案的话,右键标完未读再右键,
    // 菜单还让你"标未读",用户会以为刚才那一下没生效
    expect(fn, "已读/未读按状态切换文案").toContain(
      'done ? t("batch.unread") : t("batch.read")',
    );
    expect(fn, "收藏/取消收藏按状态切换").toContain(
      'clip.starred ? t("detail.unstar") : t("batch.star")',
    );
    // preventDefault 必须有:不拦的话,WebView 的默认右键菜单和我们的
    // 菜单会一起弹出来
    expect(fn, "得拦掉 WebView 默认右键菜单").toContain("event.preventDefault()");
    expect(fn, "得把屏幕坐标交给菜单").toContain(
      "openContextMenuAt(event.clientX, event.clientY, items)",
    );
  });

  it("删除排最后、标红;菜单调的是真实操作", () => {
    const fn = body(mainSrc, "function openClipContextMenu(event: MouseEvent, clip: ClipSummary)");
    // 最危险的放最难点到的地方:删除必须是最后一项且带 danger
    const deleteAt = fn.indexOf('t("batch.delete")');
    const dangerAt = fn.indexOf("danger: true");
    expect(deleteAt, "删除项得在").toBeGreaterThan(-1);
    expect(dangerAt, "删除项得标红").toBeGreaterThan(-1);
    expect(dangerAt).toBeGreaterThan(deleteAt);
    // 菜单调的是真的操作函数,不是摆设
    expect(fn, "已读切换走 toggleRead").toContain("void toggleRead(clip, !clip.read)");
    expect(fn, "删除走 trashClip").toContain("void trashClip(clip.filename)");
    expect(fn, "文件位置走 reveal").toContain(
      "void openClipFile(clip.filename, true)",
    );
  });

  it("menu.ts 只调真实后端:打开原文带 scheme 校验的前缀判断", () => {
    const fn = body(mainSrc, "function openClipContextMenu(event: MouseEvent, clip: ClipSummary)");
    // 没有 url 的剪藏不显示「打开原文」——和详情页同一条规则
    expect(fn, "得先判有没有 url").toContain("/^https?:\\/\\//i.test(clip.url)");
    expect(fn, "打开原文走 openUrl").toContain("void api.openUrl(clip.url)");
  });
});
