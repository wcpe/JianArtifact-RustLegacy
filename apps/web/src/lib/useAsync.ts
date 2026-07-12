// 通用异步资源加载 hook：统一 加载 / 错误 / 数据 三态与手动重载。
// 约定：传入的 fn 必须用 useCallback 记忆，其依赖变化即触发重新加载。
import { useCallback, useEffect, useState } from 'react';
import { ApiClientError } from '../api/client';

// 归一为 ApiClientError：非契约异常兜底为状态 0 的未知错误
function toApiError(error: unknown): ApiClientError {
  if (error instanceof ApiClientError) return error;
  return new ApiClientError(0, 'UNKNOWN', '请求发生未知错误');
}

// 异步资源状态：数据、错误、加载中标志与重载动作
export interface AsyncResource<T> {
  data: T | null;
  error: ApiClientError | null;
  loading: boolean;
  reload: () => void;
}

// 加载一个异步资源；fn 变化或调用 reload 时重新拉取，组件卸载后丢弃迟到结果
export function useAsync<T>(fn: () => Promise<T>): AsyncResource<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<ApiClientError | null>(null);
  const [loading, setLoading] = useState(true);
  const [nonce, setNonce] = useState(0);

  // 递增 nonce 触发 effect 重跑，实现幂等的手动重载
  const reload = useCallback(() => setNonce((value) => value + 1), []);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);
    fn()
      .then((value) => {
        if (!active) return;
        setData(value);
        setLoading(false);
      })
      .catch((err: unknown) => {
        if (!active) return;
        setError(toApiError(err));
        setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [fn, nonce]);

  return { data, error, loading, reload };
}

// 供页面在 catch 分支复用的错误归一函数
export { toApiError };
