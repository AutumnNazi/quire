/**
 * 后端送回来的错误。**只有代号和参数,没有现成的句子**——
 * 界面自己拿代号去查词典,英文用户看见的才也是英文。
 * 字段名与 Rust 侧 `WireError` 一致。
 */
export interface WireError {
  code: string;
  /** 文案里的具名占位符。 */
  args: Record<string, string>;
}

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
  /** 用户手动标的"值得回头看"。和归档是两码事:归档是读完挪走,收藏是一直留着 */
  starred: boolean;
  /** 读到哪儿了,0–1。没写过就是 0。 */
  progress: number;
  tags: string[];
  /** 用户自己写的批注。空串就是没写。 */
  note: string;
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
  /** `snippet` 里哪几段是命中的,`[起, 止)` 是**字符**下标。
   *
   *  单独一栏而不是往 snippet 里塞 `<mark>`:正文里可能有真 HTML,
   *  塞进去之后这份片段既不能当纯文本用,也没法直接进导出文件 */
  marks: Array<[number, number]>;
  /** 标题里命中的那几段 */
  titleMarks: Array<[number, number]>;
  /** **这条是「差一个字」找出来的,不是原样命中的。**
   *
   *  界面上必须标出来,而且**排在标准结果后面**。把两种混在一起不给区分,
   *  用户点进去发现不是那篇,连着错两三次他就不搜了——那比一开始就搜不到更糟。
   *  这类结果一律不高亮:我们不知道用户打错的是哪个字 */
  fuzzy?: boolean;
  score: number;
}

/** 详情 = 摘要 + 正文。Rust 侧用 `#[serde(flatten)]` 拍平的。 */
export interface ClipContent extends ClipSummary {
  body: string;
  /** 刚读到的这份内容的指纹。写回去的时候带上,后端写前比一次——
   *  对不上就是磁盘上那份已经被别处改过了,这时候写等于拿旧的盖掉新的。
   *  `undefined` 只在极老的库里出现,当"没指纹、不拦"处理 */
  stamp?: string;
}

export interface VaultInfo {
  path: string;
  /** 剪贴板监控是否开着。默认关,读界面时得把这个状态显示出来,
   *  不然用户不知道自己被监听着。 */
  watching: boolean;
  /** 剪藏库建不出来时的原因。没有它的话,只读盘或网盘掉线时界面会是个
   *  干干净净的空库、连红条都没有,用户只会以为自己的剪藏全没了。 */
  problem?: WireError;

  /** 问过用户"要不要开剪贴板监控"了没有。空库那一屏上摆不摆
   *  「要不要开启」那个按钮全看它——用户拒绝过一次之后再问是骚扰 */
  watchAsked: boolean;}

/** 回收站里的一条。字段名与 Rust 侧 `TrashItem` 一致。 */
export interface TrashItem {
  filename: string;
  /** 读不出 frontmatter 时是 `null`。**照样显示、照样能彻底删**,
   *  只显示解析得动的那些,用户会以为回收站空了,而实际上有东西清不掉。 */
  summary: ClipSummary | null;
  /** 报出来是让用户判断值不值得留着——回收站不是免费的,它占着磁盘。 */
  /** 拿不到就是 null。发 0 过去,界面会写"删掉能省 0 B",用户会以为
   *  自己算错了磁盘占用。 */
  sizeBytes: number | null;
  /** 它已经在隔离区里了吗。**界面上必须分开说**:回收站里的还能一键翻回,
   *  隔离区里的要过两道手。分不清这两者,「彻底删除」就还是闭眼签字 */
  quarantined?: boolean;
}

export interface TrashListing {
  items: TrashItem[];
  /** 隔离区保留天数。由 Rust 侧的 `QUARANTINE_DAYS` 下发,前端不自己写死——
   *  界面上"30 天后自动清理"那句话得跟着后端常量走 */
  quarantineDays: number;
}

/** 一批操作的结果。**批量最容易出的事就是"悄悄少做了一半"**:
 *  一次改 30 篇,中间有一篇被占用,用户看到的还是"操作成功"。
 *  所以 `failed` 非空就必须显示出来,不能只报成功那几条。 */
/** 一次搜索的结果。`skipped` 是**搜不到的那些文件**,不是"没搜到的东西"——
 *  两件事混在一起,用户看到空列表就只会以为自己记错了。 */
export interface SearchOutcome {
  hits: SearchHit[];
  skipped: Array<{ filename: string; reason: string }>;
}

/** 抓回来的页面。字段名与 Rust 侧 `FetchedPage` 一致 */
export interface FetchedPage {
  /** 页面的 HTML。**整份送进 webview**,正文抽取在前端做——
   *  抽取要 DOM,而 DOM 只存在于 webview 里 */
  html: string;
  /** 最终地址。页面可能 301 跳到别处,而正文里的相对链接要按它解析 */
  url: string;
}

/** 库里有多少东西没做完。**三样都空就是什么都没欠**,
 *  界面上就不摆那个入口 —— 一个永远在那儿、点进去写着「一切都好」的
 *  按钮是给人添乱的 */
export interface LibraryStatus {
  totalClips: number;
  /** 图还指着外站的。**这些导出到别的机器上会是裂图** */
  remoteImages: Array<{ filename: string; title: string; count: number }>;
  /** 有能抓的地址、正文却短得不像全文的。多半是当初抓取失败 */
  missingFulltext: Array<{ filename: string; title: string }>;
  /** 读不出的 `.md`。要自己去修的是这些文件 */
  unreadable: Array<{ filename: string; reason: string }>;
}

export interface BatchReport {
  succeeded: string[];
  failed: Array<{ filename: string; reason: WireError }>;
}

/** 限定搜索范围。字段名与 Rust 侧 `Scope` 一致。 */
export type SearchScope = "any" | "title" | "body" | "tag" | "note";

/** 从别的工具导入的结果。字段名与 Rust 侧 `DataImportReport` 一致。
 *
 *  **三类分开报,因为它们对用户是完全不同的三件事。** 合成一句
 *  "导入完成 N 篇"的话,用户分不清那 N 篇是不是他以为的那些——尤其重复那部分,
 *  他导进去的文件全都在库里已经有了,那是他最该知道的事 */
export interface DataImportReport {
  imported: string[];
  duplicates: string[];
  failed: Array<{ row: number; reason: string }>;
}

/** 导出成文件夹的结果。字段名与 Rust 侧 `ExportFolder` 一致。
 *
 *  **图片数必须报出来。** 用户把文件夹拷到另一台机器上之后才发现少了图,
 *  那时候他已经没法回头查了 */
export interface ExportFolder {
  markdown: string;
  images: number;
  failedImages: number;
}

/** 标签栏上的一项。字段名与 Rust 侧 `TagCount` 一致。 */
export interface TagCount {
  tag: string;
  count: number;
}

/** 标签改名的结果。字段名与 Rust 侧 `TagRenameReport` 一致。 */
export interface TagRenameReport {
  /** 真的动过的篇数。**不带旧标签的那些不算**,否则报告会被撑爆 */
  changed: number;
  /** 没改成的,连原因一起 */
  failed: Array<{ filename: string; reason: WireError }>;
}

/** 导入结果。两边的文件名**不是一回事**:`report` 里是用户源文件夹里的原名
 *  (报错要指得准),`imported` 里是落进剪藏库之后的新文件名。 */
export interface ImportReport {
  report: BatchReport;
  imported: string[];
  /** 压根没读进去的目录。不报出来的话,用户只会看到"导入完成 240 篇",
   *  以为全导完了,几天后才发现少了东西。 */
  unreadableDirs?: string[];
}

/** 迁移完成后后端告诉前端的结果。「拷过去多少、跳过多少」要如实给用户看:
 *  迁移是数据级动作,用户有权知道新库里到底装了多少自己的东西 */
export interface VaultMigrated {
  /** 搬过去的剪藏篇数。**只数 `.md`**——用户问的是"几篇" */
  clips: number;
  /** 跟着走的图片和附件个数,单独报:混进篇数里两篇能报成二十几篇 */
  assets: number;
  skipped: number;
}
