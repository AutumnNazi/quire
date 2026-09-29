import { invoke } from "@tauri-apps/api/core";
import type {
  ClipboardCapture,
  ClipInput,
  SavedClip,
} from "./clipboard";
import type { ClipContent, ClipSummary, ScanResult, SearchHit, VaultInfo, WeekDigest } from "./types";

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

  // 改已读/归档。read 或 archived 传 undefined 表示这一项不动,
  // 不能传 null——Rust 侧那边是 Option<bool>,null 过来会变成"没写"以外的乱东西。
  setClipFlags: (filename: string, read?: boolean, archived?: boolean) =>
    invoke<ClipSummary>("set_clip_flags", { filename, read: read ?? null, archived: archived ?? null }),
  weeklyDigest: (weeks?: number) => invoke<WeekDigest[]>("weekly_digest", { weeks: weeks ?? null }),
  // 返回 null 表示用户在保存对话框点了取消,那不是故障。
  exportVault: () => invoke<string | null>("export_vault"),
  captureClipboard: () => invoke<ClipboardCapture>("capture_clipboard"),
  saveClip: (input: ClipInput) => invoke<SavedClip>("save_clip", { input }),
  setClipboardWatch: (enabled: boolean) =>
    invoke<VaultInfo>("set_clipboard_watch", { enabled }),

  pickVault: () => invoke<VaultInfo | null>("pick_vault"),
  openVaultFolder: () => invoke<void>("open_vault_folder"),
};
