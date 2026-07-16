import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { Navigate, useLocation } from 'react-router-dom';
import { ErrorState, Skeleton } from '@jianartifact/ui';
import { setupApi } from '../api/endpoints';
import { AuthCardLayout } from '../components/AuthCardLayout';
import { SetupPage } from './SetupPage';

interface SetupGateProps {
  setupToken: string | null;
  children: ReactNode;
}

type GateState = 'loading' | 'error' | 'required' | 'ready';

async function readSetupRequired(): Promise<boolean> {
  const status = await setupApi.status();
  if (typeof status.setup_required !== 'boolean') throw new Error('初始化状态响应无效');
  return status.setup_required;
}

export function SetupGate({ setupToken, children }: SetupGateProps) {
  const location = useLocation();
  const [state, setState] = useState<GateState>('loading');
  const mounted = useRef(false);
  const requestVersion = useRef(0);

  const isCurrentRequest = useCallback(
    (version: number) => mounted.current && requestVersion.current === version,
    [],
  );

  const loadStatus = useCallback(async () => {
    const version = ++requestVersion.current;
    setState('loading');
    try {
      const required = await readSetupRequired();
      if (isCurrentRequest(version)) setState(required ? 'required' : 'ready');
    } catch {
      if (isCurrentRequest(version)) setState('error');
    }
  }, [isCurrentRequest]);

  const confirmSetupComplete = useCallback(async (): Promise<boolean> => {
    const version = ++requestVersion.current;
    try {
      const required = await readSetupRequired();
      if (!isCurrentRequest(version)) return false;
      if (required) {
        setState('required');
        return false;
      }
      setState('ready');
      return true;
    } catch {
      return false;
    }
  }, [isCurrentRequest]);

  useEffect(() => {
    mounted.current = true;
    void loadStatus();
    return () => {
      mounted.current = false;
      requestVersion.current += 1;
    };
  }, [loadStatus]);

  if (state === 'loading') {
    return (
      <AuthCardLayout title="正在启动" description="正在检查实例是否需要创建首名管理员。">
        <div
          role="status"
          aria-label="正在检查初始化状态"
          aria-busy="true"
          className="flex min-h-64 flex-col justify-center gap-3"
        >
          <Skeleton className="h-5 w-full" />
          <Skeleton className="h-5 w-4/5" />
          <Skeleton className="h-11 w-full" />
        </div>
      </AuthCardLayout>
    );
  }

  if (state === 'error') {
    return (
      <AuthCardLayout title="无法启动" description="初始化状态确认失败，应用暂时不会继续加载。">
        <div className="flex min-h-64 items-center">
          <ErrorState
            title="状态检查失败"
            message="无法检查首启初始化状态，请稍后重试。"
            onRetry={() => void loadStatus()}
          />
        </div>
      </AuthCardLayout>
    );
  }

  if (state === 'required') {
    if (location.pathname !== '/setup') return <Navigate to="/setup" replace />;
    return <SetupPage setupToken={setupToken} onSetupComplete={confirmSetupComplete} />;
  }

  if (location.pathname === '/setup') return <Navigate to="/login" replace />;
  return children;
}
