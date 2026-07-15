// 仓库接入片段（M1-FR-020）：按格式罗列可复制的客户端配置片段，凭据一律占位不含真实令牌。
import { useCallback, useState } from 'react';
import { Button, ErrorState, Skeleton } from '@jianartifact/ui';
import { Check, Copy } from 'lucide-react';
import { repoApi } from '../../api/endpoints';
import { useAsync } from '../../lib/useAsync';

type CopyFeedback = { kind: 'success' | 'error'; message: string } | null;

export function UsagePanel({ name }: { name: string }) {
  const loader = useCallback(() => repoApi.usage(name), [name]);
  const { data, error, loading, reload } = useAsync(loader);
  const [feedback, setFeedback] = useState<CopyFeedback>(null);

  const copySnippet = async (title: string, content: string) => {
    try {
      await navigator.clipboard.writeText(content);
      setFeedback({ kind: 'success', message: `已复制 ${title}` });
    } catch {
      setFeedback({ kind: 'error', message: `复制 ${title}失败，请手动复制` });
    }
  };

  if (loading) {
    return <Skeleton className="h-40 w-full" />;
  }
  if (error) {
    return <ErrorState message={error.message} onRetry={reload} />;
  }
  return (
    <div className="flex flex-col gap-4">
      {feedback ? (
        <p
          role={feedback.kind === 'success' ? 'status' : 'alert'}
          className={
            feedback.kind === 'success' ? 'text-sm text-green-700' : 'text-sm text-destructive'
          }
        >
          {feedback.message}
        </p>
      ) : null}
      {(data ?? []).map((snippet) => (
        <div key={snippet.title} className="flex flex-col gap-2">
          <div className="flex items-center justify-between gap-2">
            <h5 className="text-sm font-medium">{snippet.title}</h5>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              aria-label={`复制 ${snippet.title}`}
              onClick={() => void copySnippet(snippet.title, snippet.content)}
            >
              {feedback?.kind === 'success' && feedback.message === `已复制 ${snippet.title}` ? (
                <Check className="size-4" aria-hidden />
              ) : (
                <Copy className="size-4" aria-hidden />
              )}
              复制
            </Button>
          </div>
          <pre className="overflow-x-auto rounded-md border bg-muted p-3 text-xs">
            {snippet.content}
          </pre>
        </div>
      ))}
    </div>
  );
}
