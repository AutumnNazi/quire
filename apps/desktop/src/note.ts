/**
 * 详情页的批注框。
 *
 * 单独成个模块,是因为这段代码有一条不写测试就想不到的规矩:**保存必须
 * 排队**。批注是边打字边存的,用户从一篇跳到另一篇,两个保存请求同时在飞,
 * 后回来的那个是**旧**的——照单全收,界面就会显示一段用户早就改掉的字,
 * 而文件里是新的。对不上,而且用户看不出哪儿错了。
 *
 * 所以这里串行化:同一时刻只有一个保存请求在飞,期间的新内容记在
 * `pending`,等这个请求落地了再存一次,永远以用户最后写的那版为准。
 */

export interface NoteTarget {
  filename: string;
  note: string;
}

export interface NoteEditorOptions {
  onSave: (filename: string, note: string) => Promise<void>;
  onError: (error: unknown) => void;
  /**
   * 存成功了要告诉外面一声。
   *
   * 有这个回调是因为**存失败时外面会改文案**——提示行得从"自动保存"
   * 换成"没存上,原因是…"。存成功了不改回去的话,那句"没存上"就永远
   * 挂在那儿了,用户看着一条早就过去的错误,还以为刚才那段没存进去。
   */
  onSaved?: () => void;
}

/**
 * 给一个 `<textarea>` 挂上批注编辑:失焦或 Ctrl+Enter 存盘,Esc 撤回。
 *
 * 存盘失败**不冲掉用户写的字**,只把 `data-state="error"` 挂上去。
 * 冲掉等于假装成功过一次;留着字再报个错,用户才知道自己这段还没存上,
 * 重试或者复制走都是他说了算。
 */
export function attachNoteEditor(
  textarea: HTMLTextAreaElement,
  target: NoteTarget,
  options: NoteEditorOptions,
): void {
  // 后端最后确认过的值。Esc 撤回到这儿,不是撤回到"打开时的值"
  let saved = target.note;
  let inFlight: Promise<void> | null = null;
  let pending: string | null = null;
  // 框里先摆上后端给的那版。用户已经写过的话不能被这一行抹掉——
  // attachNoteEditor 也可能挂到一个正装着别的内容的框上
  if (textarea.value === "") textarea.value = target.note;

  const run = (value: string): void => {
    const request = options.onSave(target.filename, value);
    inFlight = request;
    request
      .then(() => {
        saved = value;
        textarea.removeAttribute("data-state");
        options.onSaved?.();
      })
      .catch((error: unknown) => {
        // 标出来给用户看。冲掉他写的字没用——他已经知道写不进去了,
        // 真正要的是这段话还在框里、还能复制走、还能接着改
        textarea.setAttribute("data-state", "error");
        options.onError(error);
      })
      .finally(() => {
        inFlight = null;
        // 存这版的过程中用户又改了,把最后那版接着存。
        // 哪怕这一版失败了也得接着试——不然一次网络抖动就把批注编辑器
        // 焊死了,用户后面写的字永远存不进去
        if (pending !== null) {
          const next = pending;
          pending = null;
          run(next);
        }
      });
  };

  const commit = (): void => {
    const next = textarea.value.trim();
    // 和后端的 trim 一致:「 abc 」和「abc」是同一个标题/批注,
    // 不跟着 trim 就等于把一次没改动的保存当成了改动,白惊动磁盘
    if (next === saved) return;
    if (inFlight) {
      pending = next;
      return;
    }
    run(next);
  };

  textarea.addEventListener("keydown", (event) => {
    // 批注是多行的,普通回车必须是换行,不能顺手吞掉
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      commit();
      textarea.blur();
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      pending = null;
      textarea.value = saved;
      textarea.blur();
    }
  });

  textarea.addEventListener("blur", () => {
    commit();
  });
}
