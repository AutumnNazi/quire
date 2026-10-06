/**
 * 阅读进度存盘的提示。
 *
 * **问题不是"存不上",是"存不上而用户不知道"。** 进度条还在屏幕上显示着
 * 他读到哪儿了,他读完关掉,下次打开发现回到零——他不会想到是写盘失败,
 * 只会觉得 Quire 记不住他读到哪儿。这个功能等于没有。
 *
 * 但也不能每次失败都弹一次:滚动停 1.5 秒就写一次,磁盘抖一下能连着弹
 * 二十次,那比不提示还糟。所以这里只管一件事:**同一场故障只说一次**,
 * 中间好了、后来又坏了,再说一次。
 */

/** 一次存盘结果该给用户什么。 */
export type ProgressOutcome =
  /** 正常,什么都不用说 */
  | "silent"
  /** 刚失败,而且这场故障还没报过 —— 该提示 */
  | "warn"
  /** 之前失败过,这次存上了 —— 该说一句"好了",把这事结掉 */
  | "recovered";

export interface ProgressJudge {
  /** 存盘回来了。`ok` 是这次成功没有。 */
  judge(ok: boolean): ProgressOutcome;
  /** 这场故障报过没有。只给测试和调试看。 */
  readonly warned: boolean;
}

/**
 * 建一个判官。
 *
 * 三态,不是两态:多出来的"刚恢复"是为了把话说完。报完错不说"好了",
 * 用户会一直惦记着刚才那个红字到底修没修好——下次打开才发现进度条动了,
 * 他不知道是自己读的还是程序存上的。
 */
export function createProgressJudge(): ProgressJudge {
  /** 这场故障报过没有。第一次失败要报,之后同一场的失败都不报。 */
  let broken = false;

  const judge = (ok: boolean): ProgressOutcome => {
    if (ok) {
      const wasBroken = broken;
      broken = false;
      return wasBroken ? "recovered" : "silent";
    }
    if (broken) return "silent";
    broken = true;
    return "warn";
  };

  return {
    judge,
    get warned() {
      return broken;
    },
  };
}

/** 一次还挂在定时器上、等着写出去的进度。 */
export interface PendingProgress {
  filename: string;
  value: number;
}

export interface ProgressQueue {
  /** 记下这一次。每滚一下都调,后写的盖掉先写的——用户要的是**最新位置**,
   *  不是滚动轨迹。 */
  put(next: PendingProgress): void;
  /** 拿走并清空。定时器到点、切文章、重画详情都走这里。 */
  take(): PendingProgress | null;
  /** 还有没有等着写的。没写过就调 `take` 的话会拿到 null,
   *  调用方据此跳过白写的一次盘。 */
  readonly pending: boolean;
}

/**
 * 待写进度的存放处。
 *
 * **为什么不在闭包里,而要单独拎出来。** 原来进度值只存在于
 * `trackReadingProgress` 的闭包 + 一个 `setTimeout` 里,切文章时把定时器
 * 一摘,那次值就没了。存成显式的队列,"还有没有没写出去的"就成了一个
 * 能查、能测的东西,而不是"看定时器还在不在"。
 */
export function createProgressQueue(): ProgressQueue {
  let current: PendingProgress | null = null;
  return {
    put: (next) => {
      current = next;
    },
    take: () => {
      const out = current;
      current = null;
      return out;
    },
    get pending() {
      return current !== null;
    },
  };
}