/** 剪藏摘要。字段名与 Rust 侧的 `ClipSummary` 一一对应(那边是 camelCase 序列化)。 */
export interface ClipSummary {
  id: string;
  filename: string;
  title: string;
  url: string;
  site: string;
  excerpt: string | null;
  clippedAt: string;
  read: boolean;
  archived: boolean;
  tags: string[];
}

/** 解析不了的文件。用户可能正手动编辑它们,必须让用户看见。 */
export interface UnreadableFile {
  filename: string;
  reason: string;
}

export interface ScanResult {
  clips: ClipSummary[];
  unreadable: UnreadableFile[];
}

/** 检索命中的一条。字段名与 Rust 侧 `SearchHit` 一致。 */
export interface SearchHit {
  summary: ClipSummary;
  /** 命中的正文片段,带前后省略号。 */
  snippet: string;
  score: number;
}

/** 详情 = 摘要 + 正文。Rust 侧用 `#[serde(flatten)]` 拍平的。 */
export interface ClipContent extends ClipSummary {
  body: string;
}

export interface VaultInfo {
  path: string;
  /** 剪贴板监控是否开着。默认关,读界面时得把这个状态显示出来,
   *  不然用户不知道自己被监听着。 */
  watching: boolean;
}

/** 一周的汇总,字段名与 Rust 侧 `WeekDigest` 一致。 */
export interface WeekDigest {
  isoYear: number;
  isoWeek: number;
  total: number;
  unread: number;
  read: number;
  clips: string[];
}
