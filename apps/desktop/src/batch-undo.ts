import type { BatchReport, ClipSummary } from "./types";

/**
 * 批量改动的撤销。
 *
 * **为什么必须记旧值,而不能"再改回去"。** 批量命令是"设成 X",不是
 * "翻一下"。撤销要是也发一次 `read=false`,那用户本来就标了已读的
 * 三篇会被翻成未读——撤销一次反而造出新错,而且他根本无从察觉。
 *
 * 所以动手前先把每篇**当时是什么**拍下来,撤销时按那张快照写回去。
 * 这是唯一能让"撤销"等于"回到我动手之前"的算法。
 */

/** 一篇在动手前的样子。**没被这次操作碰到的项是 `undefined`**——
 *  「只标已读」不该把归档状态也拍下来,否则用户在中间手动归档过,
 *  撤销会拿这个旧值把它盖掉,凭空造出一个他从没做过的改动 */
export interface ClipSnapshot {
  filename: string;
  read?: boolean;
  archived?: boolean;
  starred?: boolean;
  tags?: string[];
}

export interface BatchUndo {
  /** 恢复了多少篇。 */
  restored: number;
  /** 没恢复成的,带着原因。**不回滚成功的那部分是对的**——
   *  十篇里成了八篇,那八篇的改动是真的撤了,告诉用户"撤销失败"反而
   *  让他以为十篇都还在原样 */
  failed: Array<{ filename: string; reason: unknown }>;
}

/**
 * 动手之前拍快照。
 *
 * **只拍会真被改到的。** `read`/`archived`/`starred` 里没传的项
 * (比如"只标已读"不碰归档)不该进快照——不然撤销会拿一个没被改过的
 * 值去写,万一用户在这中间手动归档过,就被撤销抹掉了
 */
export function snapshotClips(
  clips: ClipSummary[],
  filenames: string[],
  fields: { read?: boolean; archived?: boolean; starred?: boolean; tags?: boolean },
): ClipSnapshot[] {
  const wanted = new Set(filenames);
  const out: ClipSnapshot[] = [];
  for (const clip of clips) {
    if (!wanted.has(clip.filename)) continue;
    const snap: ClipSnapshot = { filename: clip.filename };
    // **只在真的要改的项上取值。** 其余留 undefined,撤销时原样跳过
    if (fields.read !== undefined) snap.read = clip.read;
    if (fields.archived !== undefined) snap.archived = clip.archived;
    if (fields.starred !== undefined) snap.starred = clip.starred;
    if (fields.tags) snap.tags = [...clip.tags];
    out.push(snap);
  }
  return out;
}

/**
 * 一批快照里,哪些真的值得恢复。
 *
 * **只恢复真被改成功的那些。** 批量报告里失败的篇,它们的旧值原封不动
 * 还在文件里,把快照写回去等于做了一次没必要的写盘——而"值没变就不写"
 * 是这个库的一条规矩,别的地方破一次,这里跟着破
 */
export function restorable(snapshots: ClipSnapshot[], report: BatchReport): ClipSnapshot[] {
  const done = new Set(report.succeeded);
  const failed = new Set(report.failed.map((f) => f.filename));
  return snapshots.filter((s) => done.has(s.filename) && !failed.has(s.filename));
}

/**
 * 一次整批撤销。
 *
 * **一组一组发,不合并。** 标志和标签是两种命令,合成一个"万能撤销"
 * 就得后端开一个新接口,而那个接口只有这一处在用——为了一个撤销
 * 新增一条协议,不值。分组是这里唯一值得抽象的地方:每组的成败
 * 都要单独记账,不然 5 篇标志退回来了、3 篇标签没退回来,用户看到的
 * 只是一句"撤销完成"
 */
export async function undoBatch(
  snapshots: ClipSnapshot[],
  send: (group: ClipSnapshot[], kind: "flags" | "tags") => Promise<BatchReport>,
): Promise<BatchUndo> {
  const flags = snapshots.filter((s) => s.tags === undefined);
  const tags = snapshots.filter((s) => s.tags !== undefined);
  const failed: BatchUndo["failed"] = [];
  let restored = 0;

  for (const [group, kind] of [[flags, "flags"], [tags, "tags"]] as const) {
    if (group.length === 0) continue;
    let report: BatchReport;
    try {
      report = await send(group, kind);
    } catch (err) {
      // 整组炸了(锁中毒、网盘掉线)。**记成这一组每篇都失败**,
      // 不吞掉——用户得知道这一组没退回来,而不是看见一句"撤销完成"
      failed.push(...group.map((s) => ({ filename: s.filename, reason: err })));
      continue;
    }
    const done = new Set(report.succeeded);
    for (const s of group) {
      if (done.has(s.filename)) restored += 1;
      else failed.push({ filename: s.filename, reason: report.failed.find((f) => f.filename === s.filename)?.reason ?? "unknown" });
    }
  }
  return { restored, failed };
}