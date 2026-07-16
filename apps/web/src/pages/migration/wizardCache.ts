// 迁移向导中间态缓存：各步数据存 sessionStorage（键前缀 jianartifact.migration.），
// 供刷新/直达时按 hash 恢复步骤；缓存损坏或缺依赖数据时安全回退到源配置步。
import type { components } from '@jianartifact/devmock/schema';
import { MIGRATION_TASK_KEY, type Step } from './types';

type DiscoveryResult = components['schemas']['DiscoveryResult'];
type MigrationPlan = components['schemas']['MigrationPlan'];
type SourceKind = components['schemas']['SourceKind'];

// 各步缓存键（与在途任务键同前缀）
const PREFIX = 'jianartifact.migration.';
export const SOURCE_FORM_KEY = `${PREFIX}source`;
export const DISCOVERY_KEY = `${PREFIX}discovery`;
export const SELECTIONS_KEY = `${PREFIX}selections`;
export const PLAN_KEY = `${PREFIX}plan`;
export const STEP_KEY = `${PREFIX}step`;

// 源配置表单缓存结构（口令绝不缓存，恢复后须重新输入）
export interface SourceFormCache {
  kind: SourceKind;
  path: string;
  url: string;
  username: string;
}

// 单仓库的迁移选择：是否纳入 + 目标仓库名
export interface RepoSelection {
  included: boolean;
  target: string;
}

// mappings 勾选/改名缓存（键为源仓库名）
export type SelectionsCache = Record<string, RepoSelection>;

// 判定普通对象（JSON 解析结果的收窄入口）
function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

// 源表单缓存校验：kind 必须落在契约枚举内
export function isSourceFormCache(value: unknown): value is SourceFormCache {
  return (
    isRecord(value) &&
    (value.kind === 'native-dir' || value.kind === 'online' || value.kind === 'package') &&
    typeof value.path === 'string' &&
    typeof value.url === 'string' &&
    typeof value.username === 'string'
  );
}

// 发现结果缓存校验（只验最小依赖形状，字段细节以契约类型为准）
export function isDiscoveryResult(value: unknown): value is DiscoveryResult {
  return (
    isRecord(value) && typeof value.discovery_id === 'string' && Array.isArray(value.repositories)
  );
}

// 计划缓存校验
export function isMigrationPlan(value: unknown): value is MigrationPlan {
  return isRecord(value) && typeof value.plan_id === 'string' && Array.isArray(value.mappings);
}

// 勾选缓存校验：每项须有布尔 included 与字符串 target
export function isSelectionsCache(value: unknown): value is SelectionsCache {
  if (!isRecord(value)) return false;
  return Object.values(value).every(
    (item) =>
      isRecord(item) && typeof item.included === 'boolean' && typeof item.target === 'string',
  );
}

// 读取并校验一个缓存项；解析失败或形状不符一律视为不存在（安全回退）
export function loadCache<T>(key: string, validate: (value: unknown) => value is T): T | null {
  try {
    const raw = sessionStorage.getItem(key);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    return validate(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

// 写入一个缓存项；存储不可用（隐私模式/配额）时静默忽略——缓存仅为体验优化
export function saveCache(key: string, value: unknown): void {
  try {
    sessionStorage.setItem(key, JSON.stringify(value));
  } catch {
    // 忽略存储失败：不影响向导主流程
  }
}

// 清空向导全部缓存（含在途任务标识），「重新开始」时调用
export function clearWizardCache(): void {
  for (const key of [SOURCE_FORM_KEY, DISCOVERY_KEY, SELECTIONS_KEY, PLAN_KEY, STEP_KEY]) {
    sessionStorage.removeItem(key);
  }
  localStorage.removeItem(MIGRATION_TASK_KEY);
}

// 向导启动态：初始步骤与各步缓存数据
export interface WizardBoot {
  step: Step;
  discovery: DiscoveryResult | null;
  plan: MigrationPlan | null;
  taskId: string | null;
}

// 读取启动态。恢复优先级：有在途任务 → progress ＞ hash 或步骤缓存指定步且缓存齐 → 该步 ＞ source。
// 启动入口会清除 URL fragment，因此步骤缓存同时承担刷新恢复职责。
export function readWizardBoot(): WizardBoot {
  const taskId = localStorage.getItem(MIGRATION_TASK_KEY);
  const discovery = loadCache(DISCOVERY_KEY, isDiscoveryResult);
  const plan = loadCache(PLAN_KEY, isMigrationPlan);
  if (taskId) return { step: 'progress', discovery, plan, taskId };
  const requestedStep = window.location.hash.slice(1) || sessionStorage.getItem(STEP_KEY);
  if (requestedStep === 'discovery' && discovery) {
    return { step: 'discovery', discovery, plan, taskId };
  }
  if (requestedStep === 'plan' && discovery && plan) {
    return { step: 'plan', discovery, plan, taskId };
  }
  return { step: 'source', discovery, plan, taskId };
}
