// 迁移向导共享类型与常量：步骤枚举、终态判定与 localStorage 键。

// 向导步骤：源配置 → 发现 → 计划 → 进度 → 报告
export type Step = 'source' | 'discovery' | 'plan' | 'progress' | 'report';

// 刷新找回用的 localStorage 键：持久化在途任务标识
export const MIGRATION_TASK_KEY = 'jianartifact.migration.task';

// 迁移任务终态集合（十态中不再推进的状态）
const TERMINAL_STATUSES: ReadonlySet<string> = new Set([
  'succeeded',
  'partial',
  'failed',
  'cancelled',
  'interrupted',
]);

/** 判断任务是否已进入终态（终态即停止轮询）。 */
export function isTerminalStatus(status: string): boolean {
  return TERMINAL_STATUSES.has(status);
}
