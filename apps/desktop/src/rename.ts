/**
 * 详情页标题的就地编辑。
 *
 * 单独成个模块,不留在 `main.ts` 里,是因为这段代码有两条不看测试就
 * 想不到的规矩:按了 Esc 之后浏览器还会派发一次 blur,那次 blur
 * 绝不能当成提交;空标题得**交给后端去拒**,前端不能自己悄悄吞掉。
 * 规矩得钉在测试里,而 `main.ts` 一 import 就 boot,根本没法测。
 */

/** 改谁:文件名 + 用户当前看到的标题。 */
export interface RenameTarget {
  filename: string;
  title: string;
}

/**
 * 把标题元素换成一个输入框,回车或移开焦点算提交,Esc 算撤销。
 *
 * `onSave` 只负责"用户想干什么",存盘和报错归调用方——它抛错不归这里管。
 *
 * 返回那个输入框,方便测试和调用方拿引用。
 */
export function startRename(
  target: RenameTarget,
  heading: HTMLElement,
  label: string,
  onSave: (filename: string, title: string) => void,
): HTMLInputElement {
  const input = document.createElement("input");
  input.className = "detail-title-input";
  input.value = target.title;
  input.setAttribute("aria-label", label);
  heading.replaceWith(input);
  input.focus();
  input.select();

  // settled 是整段代码存在的理由。用户按 Esc 的时候,浏览器还会补一次
  // blur 事件;没有这道闸,「取消」会被当成「保存」——用户明明按了 Esc,
  // 标题却改了,而且改的还是他不想的那版
  let settled = false;
  const finish = (commit: boolean) => {
    if (settled) return;
    settled = true;
    input.replaceWith(heading);
    const next = input.value.trim();
    if (!commit || next === target.title) return;
    // 空标题照发不误。前端自己拦下来等于用户看着标题凭空消失,
    // 还不知道是自己刚才手滑删干净了——让后端拒,把话摆到界面上
    onSave(target.filename, next);
  };

  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      finish(true);
    } else if (event.key === "Escape") {
      event.preventDefault();
      finish(false);
    }
  });
  input.addEventListener("blur", () => finish(true));

  return input;
}
