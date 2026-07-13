// 迁移向导 · 各步异步处理态：旋转指示 + 醒目中文说明，明确告知用户后台正在处理。
// 发现 / 生成计划 / 创建任务三个操作步共用，避免各步重复实现处理态卡片。
import { Loader2 } from 'lucide-react';
import { Card, CardContent } from '@jianartifact/ui';

// 处理态卡片：message 为该操作的中文进行时说明（如“正在创建迁移任务…”）
export function StepLoading({ message }: { message: string }) {
  return (
    <Card>
      <CardContent
        role="status"
        aria-busy="true"
        className="flex items-center gap-3 text-sm text-muted-foreground"
      >
        <Loader2 className="size-5 shrink-0 animate-spin text-primary" aria-hidden />
        <span>{message}</span>
      </CardContent>
    </Card>
  );
}
