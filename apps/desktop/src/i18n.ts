/**
 * 多语言文案。**中文是源语言**——key 在这里定,英文那本跟着补。
 *
 * 不引第三方 i18n 库:全应用的文案就这一百来条,一个 `t()` 加两本词典
 * 够了,为此多背一个运行时依赖不值当(README 里明写了「少一个依赖少一份
 * 供应链负担」)。
 *
 * 三条规矩,`i18n.test.ts` 逐条盯着:
 *
 * 1. **两条语言的 key 集合必须完全一致。**少一个键英文用户就看见一串
 *    `filter.unread` 这样的代号,比中文还糟。
 * 2. **占位符必须两边同名。**英文那句漏了 `{n}`,界面就变成
 *    「Marked read as  articles」,比少个键还难发现——它不报错,只是不对。
 * 3. **词典里不许有没人用的键。**改文案时顺手留下一个没人引用的旧键,
 *    下一个人会以为那功能还在,照着改一遍。
 */

export type Locale = "zh-CN" | "en";

export const LOCALES: readonly Locale[] = ["zh-CN", "en"];

/** 语言名用它自己写,别用 `Intl.DisplayNames`——那玩意儿把 zh-CN 译成
 *  「中文(中国)」,按钮上塞不下这么长一串。 */
const NAMES: Record<Locale, string> = {
  "zh-CN": "中文",
  en: "English",
};

const CATALOG: Record<Locale, Record<string, string>> = {
  "zh-CN": {
    "lang.switch": "切换到 English",
    "lang.name": "中文",

    "toolbar.paste": "粘贴剪藏",
    "toolbar.paste.title": "把剪贴板里的内容存成 Markdown(Ctrl+V)",
    "toolbar.vaultPath.title": "剪藏目录",
    "toolbar.search.placeholder": "搜索剪藏…",
    "toolbar.search.title": "搜标题和正文。中文两字就能搜(比如「苹果」)。",
    "toolbar.filters.aria": "剪藏筛选",
    "toolbar.import": "导入",
    "toolbar.import.title": "把一个文件夹里的 Markdown 导入剪藏库。只读源文件,不会改动它们。",
    "toolbar.export": "导出",
    "toolbar.export.title": "把整个剪藏库导出成一个 Markdown 文件,Obsidian / Logseq 都能直接打开",
    "toolbar.open": "打开剪藏目录",
    "toolbar.pick": "更换目录",
    "toolbar.shortcut.title": "↑↓ 上下翻 · r 标已读 · a 归档 · / 搜索 · Ctrl+V 剪藏",
    "toolbar.shortcut.text": "<kbd>↑</kbd><kbd>↓</kbd> 翻 <kbd>r</kbd> 读完 <kbd>a</kbd> 归档 <kbd>/</kbd> 搜",

    "filter.all": "全部",
    "filter.unread": "未读",
    "filter.unread.title": "没读过、也没归档的",
    "filter.week": "每周",
    "filter.week.title": "按自然周分组,一眼看出这周积了多少",
    "filter.archived": "归档",
    "filter.archived.title": "归档过的剪藏。归档是挪到一边,不是删掉,随时能翻回来。",
    "filter.trash": "回收站",
    "filter.trash.title": "删掉的剪藏。放回来随时能翻回原位,彻底删除就没有了。",

    "watch.label": "监控剪贴板",
    "watch.title": "开启后,你在别处复制文章时会自动提示存到 Quire。默认关闭。",

    "batch.count": "已选 {n} 篇",
    "batch.read": "标已读",
    "batch.unread": "标未读",
    "batch.archive": "归档",
    "batch.delete": "删除",
    "batch.clear": "取消",

    "list.searchNone.title": "没找到",
    "list.searchNone.body": "换个词试试。",
    "list.empty.title": "剪藏库还是空的",
    "list.empty.body":
      "在任意文章页里选中正文,按 <kbd>Ctrl</kbd>+<kbd>C</kbd> 复制," +
      "<br />再回到 Quire 按 <kbd>Ctrl</kbd>+<kbd>V</kbd> 就能存进来。",
    "list.unreadEmpty.title": "没有未读了",
    "list.unreadEmpty.body": "都读过了。要看全部,点「全部」。",
    "list.archivedEmpty.title": "还没有归档",
    "list.archivedEmpty.body": "看完不打算再看的,在正文页点「归档」挪到这儿。",
    "list.none.title": "没有剪藏",
    "list.weekEmpty.title": "没有可回顾的剪藏",
    "list.weekHeader": "{year} 年第 {week} 周 — {count} 篇{suffix}",
    "list.weekUnread": " · 还有 {n} 篇没读",
    "list.weekAllRead": " · 都读完了",

    "clip.read": "已读",
    "clip.markRead": "标已读",
    "clip.markUnread.title": "点一下标回未读",
    "clip.markRead.title": "读完了,点一下标已读",

    "trash.header": "{count} 篇 · 共 {size}",
    "trash.empty.title": "回收站是空的",
    "trash.untitled": "读不出标题的剪藏",
    "trash.emptyTrash": "清空",
    "trash.emptyTrash.title": "把回收站里的全删掉,删了就找不回来了",
    "trash.restore": "放回来",
    "trash.restore.title": "放回剪藏库的原位置,文件名和图片都跟着回来",
    "trash.purge": "彻底删除",
    "trash.purge.title": "删掉就找不回来了,没有撤销",
    "trash.purge.titleWithSize": "删掉就找不回来了,没有撤销(能腾出 {size})",
    "trash.unreadable.title": "读不出内容的剪藏",
    "trash.unreadable.body":
      "这个文件的 frontmatter 坏了,可能被你手动改过。它还在回收站里,也能删掉。",

    "detail.openOriginal": "打开原文",
    "detail.progress.title": "读到哪儿了",
    "detail.archive": "归档",
    "detail.unarchive": "取消归档",
    "detail.unarchive.title": "放回全部列表",
    "detail.archive.title": "看完不打算再看的,挪到「归档」里去",
    "detail.delete": "删除",
    "detail.delete.title": "移到回收站,不是真删——放回收站里随时能捞回来",
    "detail.pickOne": "从左边选一篇",

    "toast.close": "关闭",
    "toast.undo": "撤销",
    "toast.saved": "已剪藏",
    "toast.savedImages": "已剪藏,{count} 张图片也存到本地了",
    "toast.movedToTrash": "已移到回收站",
    "toast.movedMany": "已移到回收站 {n} 篇",
    "toast.restored": "放回来了",
    "toast.purged": "彻底删除了",
    "toast.exported": "已导出",
    "toast.imported": "导入了 {n} 篇",
    "toast.clearedTrash": "清掉了 {n} 篇",
    "toast.duplicate": "这篇之前剪过了",
    "toast.duplicateOpen": "打开它",
    "toast.duplicateForce": "仍然存一份",
    "toast.watchOn": "已开启监控:复制文章后会提示保存",
    "toast.detected": "检测到:{preview}",
    "toast.detectedFallback": "剪贴板内容",
    "toast.save": "保存",
    "toast.ignore": "忽略",

    "confirm.batchDelete": "把选中的 {n} 篇移到回收站?",
    "confirm.batchDeleteAction": "移到回收站",
    "confirm.emptyTrash": "彻底删掉回收站里的 {n} 篇?删了就找不回来了",
    "confirm.emptyTrashAction": "彻底删除",

    "error.readFailed": "读不出来:{detail}",
    "error.markReadFailed": "标已读失败:{detail}",
    "error.archiveFailed": "归档失败:{detail}",
    "error.unarchiveFailed": "取消归档失败:{detail}",
    "error.trashFailed": "删除失败:{detail}",
    "error.restoreFailed": "放回失败:{detail}",
    "error.undoFailed": "撤销失败,文件还在回收站里:{detail}",
    "error.purgeFailed": "彻底删除失败:{detail}",
    "error.searchFailed": "搜不了:{detail}",
    "error.listFailed": "列不出剪藏:{detail}",
    "error.trashUnreadable": "回收站读不出来:{detail}",
    "error.emptyTrashFailed": "清空回收站失败:{detail}",
    "error.batchFailed": "{action}失败:{detail}",
    "error.importFailed": "导入失败:{detail}",
    "error.exportFailed": "导出失败:{detail}",
    "error.pickVaultFailed": "更换目录失败:{detail}",
    "error.clipboardEmpty": "剪贴板是空的,先在别处复制点内容",
    "error.clipboardNoContent": "剪贴板里没有可保存的内容",
    "error.unreadableFiles": "有 {n} 个文件读不出元数据:{names}",
    "error.vaultUnknown": "剪藏目录未知",
    "error.batchPartial": "{done},但有 {n} 篇没成功:{reason}",
    "error.emptyTrashPartial": "清掉了 {ok} 篇,但有 {n} 篇没删掉:{names}",
    "error.importNone": "一篇都没导进来。{n} 个文件被跳过,比如 {name}:{reason}",
    "error.importNoneEmpty": "那个文件夹里没有 .md 文件",
    "error.importPartial": "导入了 {ok} 篇,跳过 {n} 个:{names}",

    "error.vault.io": "读写剪藏文件失败:{detail}",
    "error.vault.unsafeFilename": "文件名不合法,已拒绝:{detail}",
    "error.vault.emptyContent": "剪藏内容为空,已拒绝",
    "error.vault.notFound": "剪藏不存在:{detail}",
    "error.vault.alreadyExists": "已经有同名剪藏了,没敢放回去:{detail}",
    "error.vault.trashFull": "回收站里挤不下了,请自己清一清:{detail}",
    "error.vault.importSkippedSelf": "这就是剪藏库自己,不用导",
    "error.vault.importSkippedDuplicate": "已经在库里了:{detail}",
    "error.vault.unreadable": "剪藏已放回,但读不出元数据:{name}({detail})",
    "error.vault.poisoned": "内部状态异常,请重启 Quire",

    "error.app.cancelled": "操作已取消",
    "error.app.internal": "出了点岔子:{detail}",
    "error.app.pickerFailed": "文件夹选择器没响应,请再试一次",
    "error.clipboard.unavailable": "读不到剪贴板内容(可能被其他程序占用,或当前平台尚未支持)",
    "error.export.badPath": "选中的不是一个可写的文件位置",
    "error.export.writeFailed": "写不出文件:{detail}",
    "error.export.openFolderFailed": "打不开剪藏目录:{detail}",
  },

  en: {
    "lang.switch": "Switch to 中文",
    "lang.name": "English",

    "toolbar.paste": "Paste & save",
    "toolbar.paste.title": "Save whatever is on the clipboard as Markdown (Ctrl+V)",
    "toolbar.vaultPath.title": "Library folder",
    "toolbar.search.placeholder": "Search…",
    "toolbar.search.title": "Searches titles and body text. Two CJK characters are enough.",
    "toolbar.filters.aria": "Filter clippings",
    "toolbar.import": "Import",
    "toolbar.import.title":
      "Import Markdown files from a folder. Source files are only read, never modified.",
    "toolbar.export": "Export",
    "toolbar.export.title":
      "Export the whole library as one Markdown file. Obsidian and Logseq open it directly.",
    "toolbar.open": "Open folder",
    "toolbar.pick": "Change folder",
    "toolbar.shortcut.title": "↑↓ move · r read · a archive · / search · Ctrl+V clip",
    "toolbar.shortcut.text": "<kbd>↑</kbd><kbd>↓</kbd> move <kbd>r</kbd> read <kbd>a</kbd> archive <kbd>/</kbd> search",

    "filter.all": "All",
    "filter.unread": "Unread",
    "filter.unread.title": "Never read, never archived",
    "filter.week": "Weekly",
    "filter.week.title": "Grouped by calendar week, so you can see how much piled up this week",
    "filter.archived": "Archived",
    "filter.archived.title": "Archived clippings. Archiving moves them aside, it never deletes them.",
    "filter.trash": "Trash",
    "filter.trash.title": "Deleted clippings. Restoring puts them back exactly where they were.",

    "watch.label": "Watch clipboard",
    "watch.title":
      "When on, copying an article elsewhere prompts you to save it to Quire. Off by default.",

    "batch.count": "{n} selected",
    "batch.read": "Mark read",
    "batch.unread": "Mark unread",
    "batch.archive": "Archive",
    "batch.delete": "Delete",
    "batch.clear": "Cancel",

    "list.searchNone.title": "Nothing found",
    "list.searchNone.body": "Try a different word.",
    "list.empty.title": "Your library is empty",
    "list.empty.body":
      "Select the text on any article page, press <kbd>Ctrl</kbd>+<kbd>C</kbd> to copy," +
      "<br />then come back to Quire and press <kbd>Ctrl</kbd>+<kbd>V</kbd> to save it.",
    "list.unreadEmpty.title": "Nothing unread",
    "list.unreadEmpty.body": "You are all caught up. Switch to “All” to see everything.",
    "list.archivedEmpty.title": "Nothing archived yet",
    "list.archivedEmpty.body": "Anything you are done with, press “Archive” on it to move it here.",
    "list.none.title": "No clippings",
    "list.weekEmpty.title": "Nothing to review",
    "list.weekHeader": "Week {week} of {year} — {count}{suffix}",
    "list.weekUnread": " · {n} still unread",
    "list.weekAllRead": " · all read",

    "clip.read": "Read",
    "clip.markRead": "Mark read",
    "clip.markUnread.title": "Click to mark unread again",
    "clip.markRead.title": "Finished reading, click to mark it read",

    "trash.header": "{count} items · {size}",
    "trash.empty.title": "The trash is empty",
    "trash.untitled": "Clipping with no readable title",
    "trash.emptyTrash": "Empty trash",
    "trash.emptyTrash.title": "Delete everything in the trash. There is no way back.",
    "trash.restore": "Restore",
    "trash.restore.title": "Put it back where it was, filename and images included",
    "trash.purge": "Delete permanently",
    "trash.purge.title": "Gone for good, no undo",
    "trash.purge.titleWithSize": "Gone for good, no undo (frees up {size})",
    "trash.unreadable.title": "Clipping with no readable content",
    "trash.unreadable.body":
      "This file has broken frontmatter, probably hand-edited. It is still in the trash, and it can still be deleted.",

    "detail.openOriginal": "Open original",
    "detail.progress.title": "How far you got",
    "detail.archive": "Archive",
    "detail.unarchive": "Unarchive",
    "detail.unarchive.title": "Put it back in the main list",
    "detail.archive.title": "Done with it, not keeping it around? Move it to Archived.",
    "detail.delete": "Delete",
    "detail.delete.title": "Moved to the trash, not really deleted — you can fish it out any time",
    "detail.pickOne": "Pick one on the left",

    "toast.close": "Close",
    "toast.undo": "Undo",
    "toast.saved": "Saved",
    "toast.savedImages": "Saved, and {count} images stored locally too",
    "toast.movedToTrash": "Moved to trash",
    "toast.movedMany": "Moved {n} to trash",
    "toast.restored": "Restored",
    "toast.purged": "Deleted permanently",
    "toast.exported": "Exported",
    "toast.imported": "Imported {n}",
    "toast.clearedTrash": "Cleared {n}",
    "toast.duplicate": "You clipped this before",
    "toast.duplicateOpen": "Open it",
    "toast.duplicateForce": "Save another copy",
    "toast.watchOn": "Clipboard watching on: copying an article will prompt you",
    "toast.detected": "Copied: {preview}",
    "toast.detectedFallback": "clipboard content",
    "toast.save": "Save",
    "toast.ignore": "Ignore",

    "confirm.batchDelete": "Move {n} selected to the trash?",
    "confirm.batchDeleteAction": "Move to trash",
    "confirm.emptyTrash": "Permanently delete {n} from the trash? There is no way back.",
    "confirm.emptyTrashAction": "Delete permanently",

    "error.readFailed": "Could not read it: {detail}",
    "error.markReadFailed": "Could not mark as read: {detail}",
    "error.archiveFailed": "Could not archive: {detail}",
    "error.unarchiveFailed": "Could not unarchive: {detail}",
    "error.trashFailed": "Could not delete: {detail}",
    "error.restoreFailed": "Could not restore: {detail}",
    "error.undoFailed": "Undo failed, the file is still in the trash: {detail}",
    "error.purgeFailed": "Could not delete permanently: {detail}",
    "error.searchFailed": "Search failed: {detail}",
    "error.listFailed": "Could not list your library: {detail}",
    "error.trashUnreadable": "Could not read the trash: {detail}",
    "error.emptyTrashFailed": "Could not empty the trash: {detail}",
    "error.batchFailed": "{action} failed: {detail}",
    "error.importFailed": "Import failed: {detail}",
    "error.exportFailed": "Export failed: {detail}",
    "error.pickVaultFailed": "Could not change the folder: {detail}",
    "error.clipboardEmpty": "The clipboard is empty. Copy something first.",
    "error.clipboardNoContent": "There is nothing on the clipboard worth saving.",
    "error.unreadableFiles": "{n} files have unreadable metadata: {names}",
    "error.vaultUnknown": "Library folder unknown",
    "error.batchPartial": "{done}, but {n} did not work: {reason}",
    "error.emptyTrashPartial": "Cleared {ok}, but {n} could not be deleted: {names}",
    "error.importNone": "Nothing was imported. {n} files were skipped, for example {name}: {reason}",
    "error.importNoneEmpty": "That folder has no .md files",
    "error.importPartial": "Imported {ok}, skipped {n}: {names}",

    "error.vault.io": "Could not read or write a clipping file: {detail}",
    "error.vault.unsafeFilename": "That filename is not allowed, refusing: {detail}",
    "error.vault.emptyContent": "The clipping is empty, refusing",
    "error.vault.notFound": "No such clipping: {detail}",
    "error.vault.alreadyExists": "A clipping with that name already exists, did not overwrite: {detail}",
    "error.vault.trashFull": "The trash is full, empty it yourself: {detail}",
    "error.vault.importSkippedSelf": "That is your own library, nothing to import",
    "error.vault.importSkippedDuplicate": "Already in your library: {detail}",
    "error.vault.unreadable": "Restored, but the metadata could not be read: {name} ({detail})",
    "error.vault.poisoned": "Internal state is broken, please restart Quire",

    "error.app.cancelled": "Cancelled",
    "error.app.internal": "Something went wrong: {detail}",
    "error.app.pickerFailed": "The folder picker did not respond, try again",
    "error.clipboard.unavailable":
      "Could not read the clipboard (another program may be holding it, or this platform is not supported yet)",
    "error.export.badPath": "That is not a writable file location",
    "error.export.writeFailed": "Could not write the file: {detail}",
    "error.export.openFolderFailed": "Could not open the library folder: {detail}",
  },
};

const STORAGE_KEY = "quire.locale";

/** 系统语言 → 我们的语言。
 *
 *  **按偏好顺序取第一个认得的,不是"列表里有没有中文"。** `navigator.languages`
 *  是用户排过序的:系统首选英文、第二顺位中文,那就该给英文——越过他的
 *  第一选择去满足第二选择,是替用户做主。
 *
 *  认不出的语言(德语、法语…)不拦着列表继续找,只在整条列表都认不出时
 *  才落到英文:一个没翻成德语的软件,给英文也比给中文强。 */
export function resolveLocale(navigatorLanguages: readonly string[]): Locale {
  for (const tag of navigatorLanguages) {
    const lower = tag.toLowerCase();
    if (lower.startsWith("zh")) return "zh-CN";
    if (lower.startsWith("en")) return "en";
  }
  return "en";
}

function readStored(): Locale | null {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    return raw === "zh-CN" || raw === "en" ? raw : null;
  } catch {
    // WebView 关了本地存储(无痕模式之类)。读不到就按系统语言来,别把
    // 用户卡在界面上
    return null;
  }
}

function resolveInitial(): Locale {
  const stored = readStored();
  if (stored) return stored;
  const langs = window.navigator.languages?.length
    ? window.navigator.languages
    : [window.navigator.language ?? ""];
  return resolveLocale(langs);
}

let current: Locale = resolveInitial();

const listeners = new Set<(locale: Locale) => void>();

export function getLocale(): Locale {
  return current;
}

export function localeName(locale: Locale = current): string {
  return NAMES[locale];
}

/** 切语言。**存进本地存储**:用户的选择不该每次开软件都要重来一遍。 */
export function setLocale(locale: Locale): void {
  if (locale === current) return;
  current = locale;
  try {
    window.localStorage.setItem(STORAGE_KEY, locale);
  } catch {
    // 存不下就只在这次会话里生效,不该因此报错
  }
  for (const fn of listeners) fn(locale);
}

export function onLocaleChange(fn: (locale: Locale) => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

const PLACEHOLDER = /\{(\w+)\}/g;

/** 取一条文案。**参数缺了就把整个 `{xxx}` 留在原处**——
 *  悄悄换成空串的话,「导入了  篇」比报错更难让人反应过来哪里错了。 */
export function t(key: string, params: Record<string, string | number> = {}): string {
  const table = CATALOG[current];
  const template = table[key] ?? CATALOG["zh-CN"][key];
  if (template === undefined) {
    // 词典里没有这个键。`i18n.test.ts` 会让这种情况过不了 CI,
    // 走到这儿说明是运行期传了个手写错的键
    return key;
  }
  return template.replace(PLACEHOLDER, (whole, name: string) => {
    const value = params[name];
    return value === undefined ? whole : String(value);
  });
}

/** 界面是语言无关的 DOM 结构 + 会变的文案,所以切语言要**整片重画**。
 *  靠 `data-i18n` 标在元素上,新加文案的人不用记得回来登记。 */
export function applyI18n(root: ParentNode): void {
  for (const node of root.querySelectorAll<HTMLElement>("[data-i18n]")) {
    node.textContent = t(node.dataset.i18n ?? "");
  }
  for (const node of root.querySelectorAll<HTMLElement>("[data-i18n-html]")) {
    // 只有词典里的静态片段走 innerHTML,用户数据一律走 textContent
    node.innerHTML = t(node.dataset.i18nHtml ?? "");
  }
  for (const node of root.querySelectorAll<HTMLElement>("[data-i18n-title]")) {
    node.title = t(node.dataset.i18nTitle ?? "");
  }
  for (const node of root.querySelectorAll<HTMLElement>("[data-i18n-aria]")) {
    node.setAttribute("aria-label", t(node.dataset.i18nAria ?? ""));
  }
  for (const node of root.querySelectorAll<HTMLElement>("[data-i18n-placeholder]")) {
    node.setAttribute("placeholder", t(node.dataset.i18nPlaceholder ?? ""));
  }
}

/** 切到另一个语言。现在只有中英两本,所以"另一个"就是全部。 */
export function otherLocale(locale: Locale = current): Locale {
  return locale === "zh-CN" ? "en" : "zh-CN";
}

/** 认出后端送回来的错误。
 *
 *  `Array.isArray` 也要查:数组也是 `object`,光查 `typeof` 会把
 *  `["a"]` 当成错误,取出来的 `code` 是 `undefined`。 */
export function isWireError(
  value: unknown,
): value is { code: string; args?: Record<string, string> } {
  return (
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    typeof (value as { code?: unknown }).code === "string"
  );
}

/** 把后端送回来的东西翻成一句人话。
 *
 *  接 `unknown` 而不是窄类型:命令的 `catch` 拿到的本来就是 `unknown`,
 *  让每个调用点自己判一遍类型,总有一处会写成 `String(err)` ——那对新的
 *  错误对象会打出 "[object Object]"。
 *
 *  **查不到代号就把代号原样返回**,绝不返回空串:空串在界面上是一片空白,
 *  用户只能对着屏幕猜软件是不是坏了,而一个看得见的 `vault.notFound`
 *  至少能让人报出"我这儿显示了一串英文代号"。 */
export function wireText(error: unknown): string {
  if (isWireError(error)) return t(`error.${error.code}`, error.args ?? {});
  if (error instanceof Error) return error.message;
  return String(error);
}
