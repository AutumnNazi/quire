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

/** 回收站里的一条。字段名与 Rust 侧 `TrashItem` 一致。 */
export interface TrashItem {
  filename: string;
  /** 读不出 frontmatter 时是 `null`。**照样显示、照样能彻底删**,
   *  只显示解析得动的那些,用户会以为回收站空了,而实际上有东西清不掉。 */
  summary: ClipSummary | null;
  /** 报出来是让用户判断值不值得留着——回收站不是免费的,它占着磁盘。 */
  sizeBytes: number;
}

export interface TrashListing {
  items: TrashItem[];
}

/** 一批操作的结果。**批量最容易出的事就是"悄悄少做了一半"**:
 *  一次改 30 篇,中间有一篇被占用,用户看到的还是"操作成功"。
 *  所以 `failed` 非空就必须显示出来,不能只报成功那几条。 */
export interface BatchReport {
  succeeded: string[];
  failed: Array<{ filename: string; reason: string }>;
}
