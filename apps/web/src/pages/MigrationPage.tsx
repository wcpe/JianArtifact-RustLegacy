// Nexus 迁移向导（M1-FR-034）：源配置 → 发现 → 计划 → 执行进度 → 报告的多步流程。
// 步骤映射 URL 锚点（#source/#discovery/#plan/#progress/#report），中间态缓存到 sessionStorage；
// 刷新找回优先级：在途任务（localStorage）→ hash 指定步且缓存齐 → 源配置。
import { useEffect, useState } from 'react';
import type { components } from '@jianartifact/devmock/schema';
import { PageHeader, cn } from '@jianartifact/ui';
import { SourceStep } from './migration/SourceStep';
import { DiscoveryStep } from './migration/DiscoveryStep';
import { PlanStep } from './migration/PlanStep';
import { ProgressStep } from './migration/ProgressStep';
import { ReportStep } from './migration/ReportStep';
import { MIGRATION_TASK_KEY, type Step } from './migration/types';
import {
  DISCOVERY_KEY,
  PLAN_KEY,
  clearWizardCache,
  readWizardBoot,
  saveCache,
} from './migration/wizardCache';

type DiscoveryResult = components['schemas']['DiscoveryResult'];
type MigrationPlan = components['schemas']['MigrationPlan'];
type TaskInfo = components['schemas']['TaskInfo'];

// 步骤条各步的展示序号与中文标签
const STEP_LABELS: { key: Step; label: string }[] = [
  { key: 'source', label: '源配置' },
  { key: 'discovery', label: '发现' },
  { key: 'plan', label: '计划' },
  { key: 'progress', label: '执行' },
  { key: 'report', label: '报告' },
];

// 步骤条：高亮当前步骤；已完成且缓存齐的前序步骤可点击回退，未到的步骤不可点
function Stepper({
  current,
  canEnter,
  onNavigate,
}: {
  current: Step;
  canEnter: (step: Step) => boolean;
  onNavigate: (step: Step) => void;
}) {
  const currentIndex = STEP_LABELS.findIndex((item) => item.key === current);
  return (
    <ol className="flex flex-wrap gap-2 text-sm">
      {STEP_LABELS.map((item, index) => {
        const clickable = index < currentIndex && canEnter(item.key);
        return (
          <li key={item.key}>
            <button
              type="button"
              disabled={!clickable && index !== currentIndex}
              onClick={() => (clickable ? onNavigate(item.key) : undefined)}
              className={cn(
                'flex items-center gap-2 rounded-md border px-3 py-1.5',
                index === currentIndex
                  ? 'border-primary bg-primary/10 font-medium text-foreground'
                  : 'text-muted-foreground',
                clickable ? 'hover:bg-accent hover:text-accent-foreground' : 'cursor-default',
              )}
            >
              <span
                className={cn(
                  'flex size-5 items-center justify-center rounded-full text-xs',
                  index <= currentIndex ? 'bg-primary text-primary-foreground' : 'bg-muted',
                )}
              >
                {index + 1}
              </span>
              {item.label}
            </button>
          </li>
        );
      })}
    </ol>
  );
}

export function MigrationPage() {
  // 挂载时一次性读取启动态（在途任务 / hash + 缓存 / 回退源配置）
  const [boot] = useState(readWizardBoot);
  const [step, setStep] = useState<Step>(boot.step);
  const [taskId, setTaskId] = useState<string | null>(boot.taskId);
  const [discovery, setDiscovery] = useState<DiscoveryResult | null>(boot.discovery);
  const [plan, setPlan] = useState<MigrationPlan | null>(boot.plan);

  // 切步时同步 URL 锚点（replaceState 不产生历史记录、不触发路由跳转）
  useEffect(() => {
    window.history.replaceState(null, '', `#${step}`);
  }, [step]);

  // 步骤可进入判定：依赖数据齐才允许（步骤条回退与恢复共用同一判据）
  const canEnter = (target: Step): boolean => {
    if (target === 'source') return true;
    if (target === 'discovery') return discovery !== null;
    if (target === 'plan') return discovery !== null && plan !== null;
    return taskId !== null;
  };

  // 从头开始：清空在途任务与全部中间态缓存
  const restart = () => {
    clearWizardCache();
    setTaskId(null);
    setDiscovery(null);
    setPlan(null);
    setStep('source');
  };

  // 发现成功：缓存结果并作废旧计划（discovery_id 已变化）
  const handleDiscovered = (result: DiscoveryResult) => {
    saveCache(DISCOVERY_KEY, result);
    sessionStorage.removeItem(PLAN_KEY);
    setDiscovery(result);
    setPlan(null);
    setStep('discovery');
  };

  // 计划生成成功：缓存计划
  const handlePlanned = (next: MigrationPlan) => {
    saveCache(PLAN_KEY, next);
    setPlan(next);
    setStep('plan');
  };

  // 任务创建成功：持久化标识并进入进度
  const handleExecuted = (task: TaskInfo) => {
    localStorage.setItem(MIGRATION_TASK_KEY, task.task_id);
    setTaskId(task.task_id);
    setStep('progress');
  };

  return (
    <div className="flex h-full flex-col gap-6 overflow-y-auto">
      <PageHeader title="Nexus 迁移" description="从 Nexus OSS 3.70.x 全量迁移仓库到本实例" />
      <Stepper current={step} canEnter={canEnter} onNavigate={setStep} />

      {step === 'source' ? <SourceStep onDiscovered={handleDiscovered} /> : null}

      {step === 'discovery' && discovery ? (
        <DiscoveryStep
          discovery={discovery}
          onBack={() => setStep('source')}
          onPlanned={handlePlanned}
        />
      ) : null}

      {step === 'plan' && plan ? (
        <PlanStep
          plan={plan}
          repositories={discovery?.repositories ?? []}
          onBack={() => setStep('discovery')}
          onExecuted={handleExecuted}
        />
      ) : null}

      {step === 'progress' && taskId ? (
        <ProgressStep taskId={taskId} onViewReport={() => setStep('report')} onRestart={restart} />
      ) : null}

      {step === 'report' && taskId ? <ReportStep taskId={taskId} onRestart={restart} /> : null}
    </div>
  );
}
