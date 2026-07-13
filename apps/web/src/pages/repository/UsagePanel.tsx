// 仓库接入片段（M1-FR-020）：按格式罗列可复制的客户端配置片段，凭据一律占位不含真实令牌。
import { useCallback } from 'react';
import { ErrorState, Skeleton } from '@jianartifact/ui';
import { repoApi } from '../../api/endpoints';
import { useAsync } from '../../lib/useAsync';

export function UsagePanel({ name }: { name: string }) {
  const loader = useCallback(() => repoApi.usage(name), [name]);
  const { data, error, loading, reload } = useAsync(loader);

  if (loading) {
    return <Skeleton className="h-40 w-full" />;
  }
  if (error) {
    return <ErrorState message={error.message} onRetry={reload} />;
  }
  return (
    <div className="flex flex-col gap-4">
      {(data ?? []).map((snippet) => (
        <div key={snippet.title} className="flex flex-col gap-2">
          <h5 className="text-sm font-medium">{snippet.title}</h5>
          <pre className="overflow-x-auto rounded-md border bg-muted p-3 text-xs">
            {snippet.content}
          </pre>
        </div>
      ))}
    </div>
  );
}
