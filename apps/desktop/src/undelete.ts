import type { BatchReport, ClipSummary, WireError } from "./types";

/**
 * 批量删除的撤销。
 *
 * 抽出来不是为了拆而拆:`main.ts` 里那套状态(clips 列表、选中项、当前打开的
 * 那一篇)没法在单测里摆出来,而"该给谁撤销""撤销失败几条"这两件事跟状态
 * 无关,恰恰是最容易写错、又最不容易被肉眼发现的地方。
 */

/** 一次撤销的结果。**成功和失败必须分开**——放回来 18 篇失败 2 篇的话,
 * 弹一句"已撤销"的话,用户会以为那 2 篇也回来了,回头找不到。 */
export interface UndeleteResult {
  /** 放回来的剪藏,已经按 `clipped_at` 排好,可以直接并进列表。 */
  restored: ClipSummary[];
  failed: Array<{ filename: string; reason: WireError }>;
}

/**
 * 该给哪些篇一个撤销按钮。
 *
 * **只给真正删掉的那几篇。** 批量删 20 篇有 2 篇本来就不在列表里(报的是
 * "没删成"),把没删成的也塞进撤销名单,用户一点撤销,后端就得为它们各报一次
 * `vault.notFound`——那两篇压根没被删过,报"放不回来"是在骗他。
 *
 * 全军覆没时返回空数组:没删成任何一篇就没有可撤销的东西,这时候弹撤销按钮
 * 点了必然报错。
 */
export function undoableFiles(report: BatchReport): string[] {
  return report.succeeded.filter((f) => !report.failed.some((x) => x.filename === f));
}

/**
 * 撤销放回多篇。
 *
 * 整批请求直接炸了(网盘掉线、内部锁中毒)才 reject,交给调用方弹红条;
 * 一篇失败不算炸,进 `failed` 由调用方决定怎么报。
 */
export async function undoTrashMany(
  filenames: string[],
  restore: (filenames: string[]) => Promise<{ report: BatchReport; clips: ClipSummary[] }>,
): Promise<UndeleteResult> {
  if (filenames.length === 0) return { restored: [], failed: [] };
  const { report, clips } = await restore(filenames);
  return { restored: clips, failed: report.failed };
}
