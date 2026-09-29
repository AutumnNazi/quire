/**
 * 测试里读源码要用的两个 Node 内置模块的最小声明。
 *
 *  **为这个装 `@types/node` 不划算**——那是一个新的外部依赖,而这里只需要
 *  `readFileSync` 和 `process.cwd` 两个函数。自报家门比拉一整个包进来轻。
 *
 *  只声明测试真正用到的东西:往后谁在这文件里加了新的 Node API,得自己
 *  补上,而不是让一整套类型悄悄变成可依赖的。
 */

declare module "node:fs" {
  export function readFileSync(path: string, encoding: string): string;
}

declare module "node:path" {
  export function resolve(...parts: string[]): string;
}

declare const process: {
  cwd(): string;
};
