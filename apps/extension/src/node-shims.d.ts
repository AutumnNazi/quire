/**
 * 测试里读词典和 manifest 用的几个 Node 内置 API 的最小声明。
 *
 * **为这个装 `@types/node` 不划算**——那是一个新的外部依赖,而这里只需要
 * `readFileSync` / `readdirSync` / `process.cwd`。自报家门比拉一整个包进来轻,
 * 桌面端的 `node-shims.d.ts` 是同一个理由。
 *
 * 只声明真正用到的东西:往后谁在这文件里加了新的 Node API,得自己补上,
 * 而不是让一整套类型悄悄变成可依赖的。
 */

declare module "node:fs" {
  export function readFileSync(path: string, encoding: string): string;
  export function readdirSync(
    path: string,
    options: { withFileTypes: true },
  ): Array<{ name: string; isDirectory(): boolean }>;
}

declare module "node:path" {
  export function resolve(...parts: string[]): string;
}

declare const process: {
  cwd(): string;
};
