import { beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { startRename } from "./rename";

const TARGET = { filename: "2026-09-28-a1b2c3d4-example-com.md", title: "网页给的标题" };

function mount(): HTMLHeadingElement {
  const heading = document.createElement("h1");
  heading.className = "detail-title";
  heading.textContent = TARGET.title;
  document.body.replaceChildren(heading);
  return heading;
}

function press(input: HTMLInputElement, key: string): void {
  input.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
}

describe("改标题", () => {
  let onSave: Mock<(filename: string, title: string) => void>;

  beforeEach(() => {
    onSave = vi.fn<(filename: string, title: string) => void>();
    document.body.replaceChildren();
  });

  it("标题变成一个填着原标题的输入框", () => {
    const heading = mount();

    const input = startRename(TARGET, heading, "标题", onSave);

    expect(input.value).toBe(TARGET.title);
    expect(input.getAttribute("aria-label")).toBe("标题");
    expect(heading.parentNode).toBeNull();
    expect(input.parentNode).toBe(document.body);
  });

  it("回车就提交新标题", () => {
    const heading = mount();
    const input = startRename(TARGET, heading, "标题", onSave);
    input.value = "我自己起的名字";

    press(input, "Enter");

    expect(onSave).toHaveBeenCalledExactlyOnceWith(TARGET.filename, "我自己起的名字");
  });

  /// 这条是整段代码存在的理由。Esc 之后浏览器必然还会补一次 blur,
  /// 那次 blur 如果被当成提交,用户按了"取消"标题却改了
  it("按了 Esc 之后紧跟着的那次 blur 不算提交", () => {
    const heading = mount();
    const input = startRename(TARGET, heading, "标题", onSave);
    input.value = "我不想改成这个";

    press(input, "Escape");
    input.dispatchEvent(new FocusEvent("blur"));

    expect(onSave).not.toHaveBeenCalled();
  });

  it("按 Esc 会把标题换回原来那个元素", () => {
    const heading = mount();
    const input = startRename(TARGET, heading, "标题", onSave);
    input.value = "算了";

    press(input, "Escape");

    expect(input.parentNode).toBeNull();
    expect(heading.parentNode).toBe(document.body);
    expect(heading.textContent).toBe(TARGET.title);
  });

  it("回车之后再 blur 也只提交一次", () => {
    const heading = mount();
    const input = startRename(TARGET, heading, "标题", onSave);
    input.value = "新标题";

    press(input, "Enter");
    input.dispatchEvent(new FocusEvent("blur"));

    expect(onSave).toHaveBeenCalledTimes(1);
  });

  it("移开焦点等于提交", () => {
    const heading = mount();
    const input = startRename(TARGET, heading, "标题", onSave);
    input.value = "新标题";

    input.dispatchEvent(new FocusEvent("blur"));

    expect(onSave).toHaveBeenCalledExactlyOnceWith(TARGET.filename, "新标题");
  });

  /// 标题是在输入框里边打边看的,回车之前一个字节都没改就提交,
  /// 等于白惊动一次磁盘写入,还顺手把自己的文件监控吵醒
  it("标题没改就不提交", () => {
    const heading = mount();
    const input = startRename(TARGET, heading, "标题", onSave);

    press(input, "Enter");

    expect(onSave).not.toHaveBeenCalled();
    expect(heading.parentNode).toBe(document.body);
  });

  it("前后空格不算改动,照样不提交", () => {
    const heading = mount();
    const input = startRename(TARGET, heading, "标题", onSave);
    input.value = `  ${TARGET.title}  `;

    press(input, "Enter");

    expect(onSave).not.toHaveBeenCalled();
  });

  it("提交前先去掉首尾空格", () => {
    const heading = mount();
    const input = startRename(TARGET, heading, "标题", onSave);
    input.value = "  好好看的标题  ";

    press(input, "Enter");

    expect(onSave).toHaveBeenCalledExactlyOnceWith(TARGET.filename, "好好看的标题");
  });

  /// 前端自己吞掉空标题,用户只会看见标题凭空消失,还不知道是自己
  /// 手滑删干净的。发出去让后端拒,才有句话告诉他为什么没改成
  it("清空了标题也照样发出去,由后端去拒", () => {
    const heading = mount();
    const input = startRename(TARGET, heading, "标题", onSave);
    input.value = "   ";

    press(input, "Enter");

    expect(onSave).toHaveBeenCalledExactlyOnceWith(TARGET.filename, "");
  });
});
