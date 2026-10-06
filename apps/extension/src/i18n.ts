/**
 * 扩展侧的文案。走 Chrome 原生 `chrome.i18n`,不是自己造一套词典。
 *
 * **为什么不用桌面端那套 `t()`**:桌面端是 WebView,没有 `chrome.i18n`,
 * 只能按 `navigator.languages` 自己挑语言。而扩展是**跟着浏览器语言**走的,
 * 装在英文浏览器里的用户就想要英文——`chrome.i18n` 干的就是这件事。
 * 两边机制不同是应该的,强行统一反而要让其中一边学一个它不需要的挑语言逻辑。
 *
 * **真正的收益在 manifest**:`name` 和 `description` 写的是 `__MSG_*__`,
 * 扩展列表和商店里显示的名字会跟着浏览器语言变。这是自己维护词典
 * 做不到的——那两处不在任何页面里,没有代码能去查表。
 */

/** 键名。跟 `_locales` 下面每份 messages.json 的键一一对应,
 *  `i18n.test.ts` 盯着这件事。 */
export type MessageKey =
  | "extension_name"
  | "extension_description"
  | "action_title"
  | "toast_saved"
  | "error_no_content"
  | "error_clipboard"
  | "error_page_blocked";

/**
 * 取一句文案。
 *
 * **查不到就返回空串,不返回键名。** 键名漏进界面的话,用户看到的是
 * `error_clipboard` 这种东西,比一句英文还难懂——英文他还能猜,下划线不能。
 * 空串配合下面的兜底,最坏结果是弹一句默认文案。
 */
export function t(key: MessageKey, substitutions: string[] = []): string {
  return chrome.i18n.getMessage(key, substitutions);
}
