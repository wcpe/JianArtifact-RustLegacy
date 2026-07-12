// 错误态占位：以 destructive 警告样式呈现错误信息，可选重试
import { TriangleAlertIcon } from 'lucide-react';

import { Alert, AlertDescription, AlertTitle } from '../ui/alert';
import { Button } from '../ui/button';

export interface ErrorStateProps {
  // 标题：可选，默认“出错了”
  title?: string;
  // 错误信息：必填，向用户说明发生了什么
  message: string;
  // 重试回调：提供时展示“重试”按钮
  onRetry?: () => void;
}

// 错误态：复用 Alert 的 role="alert" 语义与 destructive 变体
export function ErrorState({ title = '出错了', message, onRetry }: ErrorStateProps) {
  return (
    <Alert variant="destructive">
      <TriangleAlertIcon />
      <AlertTitle>{title}</AlertTitle>
      <AlertDescription>
        <p>{message}</p>
        {onRetry ? (
          <Button variant="outline" size="sm" className="mt-2" onClick={onRetry}>
            重试
          </Button>
        ) : null}
      </AlertDescription>
    </Alert>
  );
}
