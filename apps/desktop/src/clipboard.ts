/** 从系统剪贴板读到的原始素材。字段名与 Rust 侧 `ClipboardCapture` 一致。 */
export interface ClipboardCapture {
  html: string | null;
  text: string;
  url: string | null;
  /**
   * 装了 Quire 扩展时由扩展塞进 HTML 的元数据(键形如 `quire-title`)。
   * 没装扩展就是空对象——那是绝大多数用户的正常路径,不是出错。
   */
  meta: Record<string, string>;
}

/** 剪藏落盘后的结果。字段名与 Rust 侧 `SaveOutcome` 一致。
 *
 *  `duplicate` 是个正常的结局,不是错误:同一篇文章不该在列表里出现两次。
 *  界面上要给出「跳到已经剪过的那篇」的入口,而不是干巴巴一句"已存在"。 */
export type SaveOutcome =
  | { status: "saved"; id: string; filename: string }
  | { status: "duplicate"; filename: string; title: string };

/** 落盘请求体。字段名与 Rust 侧 `ClipInput` 一致(camelCase)。 */
export interface ClipInput {
  schemaVersion: number;
  url: string;
  title: string;
  siteName: string;
  author: string | null;
  excerpt: string | null;
  markdown: string;
  publishedAt: string | null;
  image: string | null;
  favicon: string | null;
}
