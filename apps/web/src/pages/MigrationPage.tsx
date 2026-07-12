// Nexus 迁移向导（M1-FR-034）：源配置 → 发现 → 计划 → 执行进度 → 报告的多步流程。
// 刷新找回：在途任务标识持久化到 localStorage，重新挂载时直接进入进度步骤。
import { useState } from 'react';
import type { components } from '@jianartifact/devmock/schema';
import { PageHeader, cn } from '@jianartifact/ui';
import { SourceStep } from './migration/SourceStep';
import { DiscoveryStep } from './migration/DiscoveryStep';
import { PlanStep } from './migration/PlanStep';
import { ProgressStep } from './migration/ProgressStep';
import { ReportStep } from './migration/ReportStep';
import { MIGRATION_TASK_KEY, type Step } from './migration/types';

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

// 步骤条：高亮当前步骤
function Stepper({ current }: { current: Step }) {
  const currentIndex = STEP_LABELS.findIndex((item) => item.key === current);
  return (
    <ol className="flex flex-wrap gap-2 text-sm">
      {STEP_LABELS.map((item, index) => (
        <li
          key={item.key}
          className={cn(
            'flex items-center gap-2 rounded-md border px-3 py-1.5',
            index === currentIndex
              ? 'border-primary bg-primary/10 font-medium text-foreground'
              : 'text-muted-foreground',
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
        </li>
      ))}
    </ol>
  );
}

export function MigrationPage() {
  // 刷新找回：挂载时若存在在途任务标识则直接进入进度步骤
  const [step, setStep] = useState<Step>(() =>
    localStorage.getItem(MIGRATION_TASK_KEY) ? 'progress' : 'source',
  );
  const [taskId, setTaskId] = useState<string | null>(() =>
    localStorage.getItem(MIGRATION_TASK_KEY),
  );
  const [discovery, setDiscovery] = useState<DiscoveryResult | null>(null);
  const [plan, setPlan] = useState<MigrationPlan | null>(null);

  // 从头开始：清空在途任务与中间态
  const restart = () => {
    localStorage.removeItem(MIGRATION_TASK_KEY);
    setTaskId(null);
    setDiscovery(null);
    setPlan(null);
    setStep('source');
  };

  // 任务创建成功：持久化标识并进入进度
  const handleExecuted = (task: TaskInfo) => {
    localStorage.setItem(MIGRATION_TASK_KEY, task.task_id);
    setTaskId(task.task_id);
    setStep('progress');
  };

  return (
    <div className="flex flex-col gap-6">
      <PageHeader title="Nexus 迁移" description="从 Nexus OSS 3.70.x 全量迁移仓库到本实例" />
      <Stepper current={step} />

      {step === 'source' ? (
        <SourceStep
          onDiscovered={(result) => {
            setDiscovery(result);
            setStep('discovery');
          }}
        />
      ) : null}

      {step === 'discovery' && discovery ? (
        <DiscoveryStep
          discovery={discovery}
          onBack={() => setStep('source')}
          onPlanned={(next) => {
            setPlan(next);
            setStep('plan');
          }}
        />
      ) : null}

      {step === 'plan' && plan ? (
        <PlanStep plan={plan} onBack={() => setStep('discovery')} onExecuted={handleExecuted} />
      ) : null}

      {step === 'progress' && taskId ? (
        <ProgressStep taskId={taskId} onViewReport={() => setStep('report')} onRestart={restart} />
      ) : null}

      {step === 'report' && taskId ? <ReportStep taskId={taskId} onRestart={restart} /> : null}
    </div>
  );
}
