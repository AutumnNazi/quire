/**
 * 快捷键清单。
 *
 * **它是给面板用的一份数据,不是给处理器用的。** 处理器里的判定照旧写在
 * 键盘回调里——把判定也改成读这张表,就得在里面塞进"什么时候该拦默认
 * 行为""输入框里算不算数"这些分支,表格会变成一段看不懂的配置
 *
 * 代价是两边会分叉,所以有一条测试**拿着这张表去核键盘回调里真判定的键**:
 * 表里写了而代码里没有 = 骗用户;代码里有而表里漏了 = 那个键等于不存在
 * (`shortcuts.test.ts`)
 *
 * **`keys` / `caps` 一一对应。** `keys` 是 `KeyboardEvent.key` 小写后的值
 * (判定用的就是它),`caps` 是界面上要显示的键面——`arrowdown` 得显示成
 * `↓`,这两个没法用同一个字符串
 */
export interface Shortcut {
  keys: string[];
  caps: string[];
  /** 文案键。面板那一行右边写的字 */
  label: string;
  /** 要不要按住 Ctrl / Cmd。显示上会带出来,判定核对时也认它 */
  mod?: boolean;
}

export interface ShortcutGroup {
  title: string;
  items: Shortcut[];
}

export const SHORTCUTS: readonly ShortcutGroup[] = [
  {
    title: "shortcut.group.read",
    items: [
      { keys: ["arrowdown", "j"], caps: ["↓", "j"], label: "shortcut.next" },
      { keys: ["arrowup", "k"], caps: ["↑", "k"], label: "shortcut.prev" },
      { keys: ["/"], caps: ["/"], label: "shortcut.search" },
      { keys: ["escape"], caps: ["Esc"], label: "shortcut.escape" },
    ],
  },
  {
    title: "shortcut.group.act",
    items: [
      { keys: ["r"], caps: ["r"], label: "shortcut.read" },
      { keys: ["a"], caps: ["a"], label: "shortcut.archive" },
      { keys: ["a"], caps: ["Ctrl", "A"], label: "shortcut.selectAll", mod: true },
    ],
  },
  {
    title: "shortcut.group.anywhere",
    items: [
      { keys: ["v"], caps: ["Ctrl", "V"], label: "shortcut.clip", mod: true },
      { keys: ["f5"], caps: ["F5"], label: "shortcut.refresh" },
      { keys: ["?"], caps: ["?"], label: "shortcut.help" },
    ],
  },
  {
    title: "shortcut.group.typing",
    items: [
      { keys: ["enter"], caps: ["Ctrl", "Enter"], label: "shortcut.saveNote", mod: true },
      { keys: ["escape"], caps: ["Esc"], label: "shortcut.dropNote" },
      { keys: ["enter"], caps: ["Enter"], label: "shortcut.confirm" },
    ],
  },
];

/** 面板上到底列了哪些键——测试拿去和键盘回调核对的。 */
export function listedKeys(): Set<string> {
  const out = new Set<string>();
  for (const group of SHORTCUTS) {
    for (const item of group.items) {
      for (const key of item.keys) out.add(key);
    }
  }
  return out;
}
