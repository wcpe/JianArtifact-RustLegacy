// 认证上下文：持有当前会话令牌与用户身份，供守卫路由与页面消费。
// 令牌持久化到 localStorage 以支持刷新页面保持登录；退出即清除。
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import { authApi } from '../api/endpoints';
import { setSessionToken } from '../api/client';

// 当前登录用户的最小身份（用于路由守卫；权威权限判定仍以后端为准）
export interface CurrentUser {
  id: number;
  username: string;
  role: 'admin' | 'user';
}

interface AuthState {
  token: string | null;
  user: CurrentUser | null;
}

interface PersistedAuthState {
  token?: string | null;
}

interface AuthContextValue {
  token: string | null;
  user: CurrentUser | null;
  isAuthenticated: boolean;
  isAdmin: boolean;
  isRestoring: boolean;
  login: (username: string, password: string) => Promise<void>;
  logout: () => Promise<void>;
  clearLocalSession: () => void;
}

const STORAGE_KEY = 'jianartifact.auth';

// 从 localStorage 只恢复令牌；用户身份始终由 auth/me 重新确认
function loadState(): AuthState {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) {
      setSessionToken(null);
      return { token: null, user: null };
    }
    const parsed = JSON.parse(raw) as PersistedAuthState;
    const token = typeof parsed.token === 'string' && parsed.token !== '' ? parsed.token : null;
    if (!token) {
      setSessionToken(null);
      localStorage.removeItem(STORAGE_KEY);
      return { token: null, user: null };
    }
    setSessionToken(token);
    return { token, user: null };
  } catch {
    setSessionToken(null);
    localStorage.removeItem(STORAGE_KEY);
    return { token: null, user: null };
  }
}

const AuthContext = createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<AuthState>(loadState);
  const [isRestoring, setIsRestoring] = useState(state.token !== null);

  const clearLocalSession = useCallback(() => {
    setSessionToken(null);
    localStorage.removeItem(STORAGE_KEY);
    setState({ token: null, user: null });
    setIsRestoring(false);
  }, []);

  useEffect(() => {
    if (!state.token || !isRestoring) return;
    let active = true;
    void authApi
      .me()
      .then((info) => {
        if (!active) return;
        const next: AuthState = {
          token: state.token,
          user: { id: info.id, username: info.username, role: info.role },
        };
        localStorage.setItem(STORAGE_KEY, JSON.stringify({ ...next, identity_source: 'me' }));
        setState(next);
        setIsRestoring(false);
      })
      .catch(() => {
        if (!active) return;
        clearLocalSession();
      });
    return () => {
      active = false;
    };
  }, [state.token, isRestoring, clearLocalSession]);

  const login = useCallback(
    async (username: string, password: string) => {
      const session = await authApi.login({ username, password });
      setSessionToken(session.token);
      try {
        const info = await authApi.me();
        const next: AuthState = {
          token: session.token,
          user: { id: info.id, username: info.username, role: info.role },
        };
        localStorage.setItem(STORAGE_KEY, JSON.stringify({ ...next, identity_source: 'me' }));
        setState(next);
      } catch (error) {
        clearLocalSession();
        throw error;
      }
    },
    [clearLocalSession],
  );

  const logout = useCallback(async () => {
    try {
      await authApi.logout();
    } finally {
      clearLocalSession();
    }
  }, [clearLocalSession]);

  const value = useMemo<AuthContextValue>(
    () => ({
      token: state.token,
      user: state.user,
      isAuthenticated: state.token !== null && state.user !== null,
      isAdmin: state.user?.role === 'admin',
      isRestoring,
      login,
      logout,
      clearLocalSession,
    }),
    [state, isRestoring, login, logout, clearLocalSession],
  );

  return <AuthContext value={value}>{children}</AuthContext>;
}

// 读取认证上下文；必须在 AuthProvider 内使用
export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error('useAuth 必须在 AuthProvider 内使用');
  return ctx;
}
