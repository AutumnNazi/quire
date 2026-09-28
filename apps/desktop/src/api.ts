import { invoke } from "@tauri-apps/api/core";
import type {
  ClipboardCapture,
  ClipInput,
  SavedClip,
} from "./clipboard";
import type { ClipContent, ScanResult, VaultInfo } from "./types";

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

  captureClipboard: () => invoke<ClipboardCapture>("capture_clipboard"),
  saveClip: (input: ClipInput) => invoke<SavedClip>("save_clip", { input }),
  setClipboardWatch: (enabled: boolean) =>
    invoke<VaultInfo>("set_clipboard_watch", { enabled }),

  pickVault: () => invoke<VaultInfo | null>("pick_vault"),
  openVaultFolder: () => invoke<void>("open_vault_folder"),
};
