import { invoke } from "@tauri-apps/api/core";
import type {
  ClipboardCapture,
  ClipInput,
  SaveOutcome,
} from "./clipboard";
import type {
  ClipContent,
  ClipSummary,
  BatchReport,
  DataImportReport,
  ExportFolder,
  FetchedPage,
  ImportReport,
  LibraryStatus,
  ScanResult,
  SearchOutcome,
  TrashListing,
  VaultInfo,
  SearchScope,
  TagCount,
  TagRenameReport,
} from "./types";

/**
 * Tauri 命令的薄封装。
 *
 * 命令名必须与 Rust 侧 `#[tauri::command]` 的函数名一致——Tauri 靠字符串匹配,
 * 改 Rust 那边的名字不会有编译错误,只会在运行时静默失败,所以这里集中一处管理。
 */
export const api = {
  vaultInfo: () => invoke<VaultInfo>("vault_info"),
  listClips: () => invoke<ScanResult>("list_clips"),
  readClip: (filename: string) => invoke<ClipContent>("read_clip", { filename }),
  // 返回值带 skipped:读不出、没 frontmatter、没有 id 的 .md 搜不到,
  // 那些文件必须跟着结果一起回来,不然用户只会以为是自己记错了
  // scope 限定只在哪些字段里找。**不传就是全字段搜**,和加这个参数之前一样
  searchClips: (query: string, limit?: number, scope?: SearchScope) =>
    invoke<SearchOutcome>("search_clips", { query, limit: limit ?? null, scope: scope ?? null }),
  // 回收站是独立视图,搜索跟着它走
  searchTrash: (query: string, limit?: number, scope?: SearchScope) =>
    invoke<SearchOutcome>("search_trash", { query, limit: limit ?? null, scope: scope ?? null }),

  // 改已读/归档/收藏。read、archived、starred 传 undefined 表示这一项不动,
  // 不能传 null——Rust 侧那边是 Option<bool>,null 过来会变成"没写"以外的乱东西
  setClipFlags: (
    filename: string,
    read?: boolean,
    archived?: boolean,
    starred?: boolean,
  ) =>
    invoke<ClipSummary>("set_clip_flags", {
      filename,
      read: read ?? null,
      archived: archived ?? null,
      starred: starred ?? null,
    }),
  // 移进回收站,不是真删。后端只搬文件,用户不点撤销也还能自己去捞。
  // 一篇就是一批里只有一篇,所以只有批量这一条命令
  trashClips: (filenames: string[]) => invoke<BatchReport>("trash_clips", { filenames }),
  setClipProgress: (filename: string, progress: number) =>
    invoke<ClipSummary>("set_clip_progress", { filename, progress }),
  setClipTags: (filename: string, tags: string[]) =>
    invoke<ClipSummary>("set_clip_tags", { filename, tags }),
  // **这三个都带 `stamp`。**读详情时后端给了这一篇的指纹,写回去时带上,
  // 它写之前比一次:对不上说明磁盘上那份已经被别处改过了(典型是在
  // Obsidian 里动过),这时候写就是拿几秒前那份盖掉用户刚敲的
  //
  // 不传就等于"没指纹、不拦"——批量路径没有编辑上下文,给不出指纹,
  // 硬造一个只会让批量操作在用户改过文件的库上全军覆没
  setClipTitle: (filename: string, title: string, stamp?: string) =>
    invoke<ClipSummary>("set_clip_title", { filename, title, stamp: stamp ?? null }),
  setClipNote: (filename: string, note: string, stamp?: string) =>
    invoke<ClipSummary>("set_clip_note", { filename, note, stamp: stamp ?? null }),
  listTags: () => invoke<TagCount[]>("list_tags"),
  setClipTagsBatch: (filenames: string[], tags: string[]) =>
    invoke<BatchReport>("set_clip_tags_batch", { filenames, tags }),
  setClipFlagsBatch: (filenames: string[], read?: boolean, archived?: boolean, starred?: boolean) =>
    invoke<BatchReport>("set_clip_flags_batch", {
      filenames,
      read: read ?? null,
      archived: archived ?? null,
      starred: starred ?? null,
    }),
  restoreClip: (filename: string) => invoke<ClipSummary>("restore_clip", { filename }),
  // 批量删除的撤销。走批量命令而不是在前端循环调 restoreClip:
  // 一圈 IPC 往返回来,中间那篇失败的话得自己拼失败清单,还得防后发先至。
  // 摘要一起回来,省得为一个撤销重扫整个剪藏库(重扫会顺手清掉刚弹的红条)
  restoreClips: (filenames: string[]) =>
    invoke<{ report: BatchReport; clips: ClipSummary[] }>("restore_clips", { filenames }),

  // 回收站:翻一遍、看内容、放回去、彻底删掉、清空
  listTrash: () => invoke<TrashListing>("list_trash"),
  readTrashClip: (filename: string) => invoke<ClipContent>("read_trash_clip", { filename }),
  purgeClip: (filename: string) => invoke<void>("purge_clip", { filename }),
  // **真删。** 全软件唯一不可撤销的操作,只认隔离区里的。
  // 空壳里没这一对,用户就是永远清不掉磁盘
  forgetClip: (filename: string) => invoke<void>("forget_clip", { filename }),
  forgetAll: () => invoke<BatchReport>("forget_all"),
  emptyTrash: () => invoke<BatchReport>("empty_trash"),
  // 返回 null 表示用户在保存对话框点了取消,那不是故障。
  exportVault: () => invoke<string | null>("export_vault"),

  /** 导出成文件夹,图片跟着走。返回 null 是用户点了取消,那不是故障 */
  exportFolder: (filenames?: string[]) =>
    invoke<ExportFolder | null>("export_folder", { filenames }),
  captureClipboard: () => invoke<ClipboardCapture>("capture_clipboard"),

  /** 自己把文章页面抓下来。**抓到的是 HTML,正文抽取在前端做**——
   *  抽取要 DOM,而 DOM 只存在于 webview 里。
   *
   *  失败不是故障,是「这个页面抓不到」:需要登录的、纯 JS 渲染的、
   *  被拦的,都属于这一类。用户复制的那段文字照样存得下来 */
  fetchArticle: (url: string) => invoke<FetchedPage>("fetch_article", { url }),

  /** 补全正文。**只换正文,frontmatter 一个字不动**——
   *  标签、批注、已读、阅读进度都是用户在这篇上花过时间的证据,
   *  一次"重试"把它们清掉,那不是帮忙 */
  setClipBody: (filename: string, markdown: string, stamp?: string) =>
    invoke<ClipSummary>("set_clip_body", { filename, markdown, stamp: stamp ?? null }),
  /** 这篇有没有一个能抓的地址。**判断放在后端**,前端不另写一份判据——
   *  两份判据迟早对不上,而对不上的后果是前端拿着乱七八糟的文字去发请求 */
  clipSourceUrl: (filename: string) => invoke<string | null>("clip_source_url", { filename }),

  /** 库里有多少东西没做完:图还指着外站的、正文短得不像全文的、读不出的文件。
   *
   *  **数的是当下的文件,不是记下来的事件。** 记"当时失败了几张"的话,
   *  用户后来自己把图补上,那条记录就过期了——而一份过期的账比没有账更坏 */
  libraryStatus: () => invoke<LibraryStatus>("library_status"),
  // force 是给用户的出口:文章更新了想重存一份时,界面上点「仍然存一份」会带上它。
  // 不给这个开关的话,判重就是个只进不出的死胡同。
  saveClip: (input: ClipInput, force = false) =>
    invoke<SaveOutcome>("save_clip", { input, force }),
  setClipboardWatch: (enabled: boolean) =>
    invoke<VaultInfo>("set_clipboard_watch", { enabled }),

  // 导入会弹目录选择器,用户取消时后端回 Err("已取消")——那不是故障
  importMarkdown: () => invoke<ImportReport>("import_markdown"),

  /** 从 Pocket / 别的工具的 CSV·JSON 导入。**返回 null 是用户点了取消**,
   *  那不是故障,不能当错误弹出去 */
  importDataFile: () => invoke<DataImportReport | null>("import_data_file"),

  // 标签改名 / 合并。`to` 已经有了就是合并(后端取并集)。
  // **失败的文件跟着报告回来**,不能只给一个「成功了几篇」——用户合并完
  // 回头发现有几篇没跟上,那几篇会一直挂着旧标签
  renameTag: (from: string, to: string) =>
    invoke<TagRenameReport>("rename_tag", { from, to }),

  // 用默认程序打开某一篇,或者在文件管理器里定位它
  openClipFile: (filename: string, reveal: boolean) =>
    invoke<void>("open_clip_file", { filename, reveal }),

  // 启动回到上次读的那篇。**存不进去不报错**——那是锦上添花,
  // 为一个配置文件失败弹红条是用红杠换一个可有可无的便利
  rememberLastRead: (filename: string) => invoke<void>("remember_last_read", { filename }),
  // 那一篇已经不在了(被删了、换了剪藏库)时返回 null,界面回列表
  lastRead: () => invoke<string | null>("last_read"),

  // 最近搜过的词,新的在前,最多 10 条。
  // **记下来是因为用户搜不到的时候想不起上次搜了什么**——他记得有那篇文章,
  // 想不起它叫什么词。存不进去不报错:那只是下次少一条建议,
  // 为这个弹红条是拿一件小事去吓一个正要找东西的人
  rememberSearch: (query: string) => invoke<void>("remember_search", { query }),
  recentSearches: () => invoke<string[]>("recent_searches"),

  pickVault: () => invoke<VaultInfo | null>("pick_vault"),
  openVaultFolder: () => invoke<void>("open_vault_folder"),
};
