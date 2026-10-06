import { beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { attachNoteEditor } from "./note";

const TARGET = { filename: "2026-09-28-a1b2c3d4-example-com.md", note: "" };

/** 手动控制 resolve 时机的 Promise,用来把两次保存的返回顺序倒过来 */
function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void; reject: (e: unknown) => void } {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("批注框", () => {
  let area: HTMLTextAreaElement;
  let onSave: Mock<(filename: string, note: string) => Promise<void>>;
  let onError: Mock<(error: unknown) => void>;

  beforeEach(() => {
    area = document.createElement("textarea");
    document.body.replaceChildren(area);
    onSave = vi.fn<(filename: string, note: string) => Promise<void>>().mockResolvedValue();
    onError = vi.fn<(error: unknown) => void>();
    attachNoteEditor(area, TARGET, { onSave, onError });
  });

  function type(value: string): void {
    area.value = value;
  }

  function press(key: string, init: KeyboardEventInit = {}): void {
    area.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init }));
  }

  function blur(): void {
    area.dispatchEvent(new FocusEvent("blur"));
  }

  it("失焦就存", () => {
    type("为什么存它");

    blur();

    expect(onSave).toHaveBeenCalledExactlyOnceWith(TARGET.filename, "为什么存它");
  });

  /// 批注是边打字边存的,一个字节没改还去写盘,不但白惊动磁盘,
  /// 还会把自己的文件监控吵醒
  it("一个字没改就不存", () => {
    blur();

    expect(onSave).not.toHaveBeenCalled();
  });

  /// 首尾空格不该算改动:用户不小心多敲了一个空格,后端 trim 之后
  /// 存进去的还是同一段字,这时候报"已保存"是在骗人
  it("只有首尾空格的变化不算改动", () => {
    type("  ");
    blur();

    expect(onSave).not.toHaveBeenCalled();
  });

  it("存之前先去掉首尾空格", () => {
    type("  为什么存它  ");

    blur();

    expect(onSave).toHaveBeenCalledExactlyOnceWith(TARGET.filename, "为什么存它");
  });

  it("Ctrl+Enter 立刻存", () => {
    type("写完了");

    press("Enter", { ctrlKey: true });

    expect(onSave).toHaveBeenCalledExactlyOnceWith(TARGET.filename, "写完了");
  });

  /// 批注是多行的,普通回车必须是换行。顺手吞掉等于告诉用户
  /// "这里写不了第二行"
  it("普通回车是换行,不存", () => {
    type("第一段");
    press("Enter");

    expect(onSave).not.toHaveBeenCalled();
  });

  it("Esc 撤回到后端确认过的值", async () => {
    type("先存一版");
    blur();
    await vi.waitFor(() => expect(onSave).toHaveBeenCalledTimes(1));

    type("改到一半又后悔");
    press("Escape");

    expect(area.value).toBe("先存一版");
  });

  /// 这条是这个模块存在的理由。用户从一篇跳到另一篇,两个保存请求
  /// 同时在飞,后回来的那个是旧的——照单全收,界面就显示一段用户
  /// 早就改掉的字,而文件里是新的
  it("存盘期间又改了,最后写的那版才落盘", async () => {
    const first = deferred<void>();
    onSave.mockReset();
    onSave.mockReturnValueOnce(first.promise).mockResolvedValue(undefined);

    type("第一版");
    blur();
    type("第二版");
    blur();

    // 第二个请求得排队,不能并发
    expect(onSave).toHaveBeenCalledTimes(1);

    first.resolve();
    await vi.waitFor(() => expect(onSave).toHaveBeenCalledTimes(2));
    expect(onSave).toHaveBeenLastCalledWith(TARGET.filename, "第二版");
  });

  /// 连着改三版,中间那版**不该**存。用户敲「一」→「一二」→「一二三」的
  /// 功夫可能只有半秒,把半秒前的那个中间态也落一次盘,纯属白惊动磁盘。
  /// 排队只保证**顺序**和**最终值**,不保证一版不漏
  it("连改三次只存头尾两版,最后落盘的是最后一次", async () => {
    const first = deferred<void>();
    onSave.mockReset();
    onSave.mockReturnValueOnce(first.promise).mockResolvedValue(undefined);

    type("一");
    blur();
    type("一二");
    blur();
    type("一二三");
    blur();

    first.resolve();
    await vi.waitFor(() => expect(onSave).toHaveBeenCalledTimes(2));
    expect(onSave.mock.calls.map((c) => c[1])).toEqual(["一", "一二三"]);
  });

  /// 存失败冲掉用户写的字,等于假装成功过一次。用户看着框里空空的,
  /// 根本不知道刚才那段去哪了
  it("存失败会报错,而且不冲掉用户写的字", async () => {
    const boom = new Error("磁盘满了");
    onSave.mockRejectedValue(boom);
    type("这段还没存上");

    blur();
    await vi.waitFor(() => expect(onError).toHaveBeenCalledExactlyOnceWith(boom));

    expect(area.value).toBe("这段还没存上");
    expect(area.getAttribute("data-state")).toBe("error");
  });

  /// 一次网络抖动就把批注编辑器焊死的话,用户后面写的字永远存不进去
  it("存失败之后还能接着存", async () => {
    onSave.mockRejectedValueOnce(new Error("磁盘满了")).mockResolvedValue(undefined);
    type("第一次失败");

    blur();
    await vi.waitFor(() => expect(onError).toHaveBeenCalledTimes(1));

    type("第二次成功");
    blur();
    await vi.waitFor(() => expect(onSave).toHaveBeenCalledTimes(2));

    await vi.waitFor(() => expect(area.hasAttribute("data-state")).toBe(false));
  });

  it("存盘失败时,排队的那个版本还是会接着试", async () => {
    const first = deferred<void>();
    onSave.mockReset();
    onSave.mockReturnValueOnce(first.promise).mockResolvedValue(undefined);

    type("第一版");
    blur();
    type("第二版");
    blur();

    first.reject(new Error("断了"));
    await vi.waitFor(() => expect(onSave).toHaveBeenCalledTimes(2));
    expect(onSave).toHaveBeenLastCalledWith(TARGET.filename, "第二版");
  });

  /// 存失败时外面把提示行换成了"没存上"。存成功了不换回去,
  /// 那句话就一直挂在那儿,用户以为刚才那段没存进去
  it("存成功了要通知外面一声,好把错误提示换掉", async () => {
    // 不能复用模块级那个 area:它已经挂了编辑器,再挂一个会存两次。
    // 这里单开一个框,自己派发事件
    const box = document.createElement("textarea");
    const onSaved = vi.fn();
    attachNoteEditor(box, TARGET, { onSave, onError, onSaved });
    onSave.mockRejectedValueOnce(new Error("断了")).mockResolvedValue(undefined);
    const lost = () => box.dispatchEvent(new FocusEvent("blur"));

    box.value = "第一次失败";
    lost();
    await vi.waitFor(() => expect(onError).toHaveBeenCalledTimes(1));
    expect(onSaved).not.toHaveBeenCalled();

    box.value = "第二次成功";
    lost();

    await vi.waitFor(() => expect(onSaved).toHaveBeenCalledTimes(1));
  });

  it("打开时后端带着批注,框里就得是它", () => {
    const box = document.createElement("textarea");
    const save = vi.fn<(f: string, n: string) => Promise<void>>().mockResolvedValue();
    attachNoteEditor(box, { filename: "x.md", note: "上次写的话" }, { onSave: save, onError });

    expect(box.value).toBe("上次写的话");
  });

  /// 用户可能正在框里打字的时候,界面因为别的原因重画了详情。
  /// 这时候挂上去的编辑器不能把人家写了一半的字抹掉
  it("框里已经有内容时不覆盖", () => {
    const box = document.createElement("textarea");
    box.value = "正在写…";
    attachNoteEditor(box, { filename: "x.md", note: "上次写的话" }, { onSave: vi.fn(), onError: vi.fn() });

    expect(box.value).toBe("正在写…");
  });
});
