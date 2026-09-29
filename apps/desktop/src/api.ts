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
  ImportReport,
  ScanResult,
  SearchHit,
  TrashListing,
  VaultInfo,
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
  searchClips: (query: string, limit?: number) =>
    invoke<SearchHit[]>("search_clips", { query, limit: limit ?? null }),
  // 回收站是独立视图,搜索跟着它走
  searchTrash: (query: string, limit?: number) =>
    invoke<SearchHit[]>("search_trash", { query, limit: limit ?? null }),

  // 改已读/归档。read 或 archived 传 undefined 表示这一项不动,
  // 不能传 null——Rust 侧那边是 Option<bool>,null 过来会变成"没写"以外的乱东西。
  setClipFlags: (filename: string, read?: boolean, archived?: boolean) =>
    invoke<ClipSummary>("set_clip_flags", { filename, read: read ?? null, archived: archived ?? null }),
  // 移进回收站,不是真删。后端只搬文件,用户不点撤销也还能自己去捞。
  // 一篇就是一批里只有一篇,所以只有批量这一条命令
  trashClips: (filenames: string[]) => invoke<BatchReport>("trash_clips", { filenames }),
  setClipProgress: (filename: string, progress: number) =>
    invoke<ClipSummary>("set_clip_progress", { filename, progress }),
  setClipFlagsBatch: (filenames: string[], read?: boolean, archived?: boolean) =>
    invoke<BatchReport>("set_clip_flags_batch", {
      filenames,
      read: read ?? null,
      archived: archived ?? null,
    }),
  restoreClip: (filename: string) => invoke<ClipSummary>("restore_clip", { filename }),

  // 回收站:翻一遍、看内容、放回去、彻底删掉、清空
  listTrash: () => invoke<TrashListing>("list_trash"),
  readTrashClip: (filename: string) => invoke<ClipContent>("read_trash_clip", { filename }),
  purgeClip: (filename: string) => invoke<void>("purge_clip", { filename }),
  emptyTrash: () => invoke<BatchReport>("empty_trash"),
  // 返回 null 表示用户在保存对话框点了取消,那不是故障。
  exportVault: () => invoke<string | null>("export_vault"),
  captureClipboard: () => invoke<ClipboardCapture>("capture_clipboard"),
  // force 是给用户的出口:文章更新了想重存一份时,界面上点「仍然存一份」会带上它。
  // 不给这个开关的话,判重就是个只进不出的死胡同。
  saveClip: (input: ClipInput, force = false) =>
    invoke<SaveOutcome>("save_clip", { input, force }),
  setClipboardWatch: (enabled: boolean) =>
    invoke<VaultInfo>("set_clipboard_watch", { enabled }),

  // 导入会弹目录选择器,用户取消时后端回 Err("已取消")——那不是故障
  importMarkdown: () => invoke<ImportReport>("import_markdown"),

  pickVault: () => invoke<VaultInfo | null>("pick_vault"),
  openVaultFolder: () => invoke<void>("open_vault_folder"),
};
