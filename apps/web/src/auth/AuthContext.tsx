// 认证上下文：持有当前会话令牌与用户身份，供守卫路由与页面消费。
// 令牌持久化到 localStorage 以支持刷新页面保持登录；退出即清除。
import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from 'react';
import { authApi } from '../api/endpoints';
import { setSessionToken } from '../api/client';

// 当前登录用户的最小身份（用于路由守卫；权威权限判定仍以后端为准）
export interface CurrentUser {
  username: string;
  role: 'admin' | 'user';
}

interface AuthState {
  token: string | null;
  user: CurrentUser | null;
}

interface AuthContextValue {
  token: string | null;
  user: CurrentUser | null;
  isAuthenticated: boolean;
  isAdmin: boolean;
  login: (username: string, password: string) => Promise<void>;
  logout: () => Promise<void>;
}

const STORAGE_KEY = 'jianartifact.auth';

// 从 localStorage 恢复登录态（页面刷新后保持）
function loadState(): AuthState {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { token: null, user: null };
    const parsed = JSON.parse(raw) as AuthState;
    setSessionToken(parsed.token);
    return parsed;
  } catch {
    return { token: null, user: null };
  }
}

const AuthContext = createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<AuthState>(loadState);

  const login = useCallback(async (username: string, password: string) => {
    const session = await authApi.login({ username, password });
    // mock 与真实后端一致：登录成功后令牌用于后续鉴权；用户身份由用户名推导供路由守卫
    const user: CurrentUser = { username, role: username === 'admin' ? 'admin' : 'user' };
    setSessionToken(session.token);
    const next: AuthState = { token: session.token, user };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
    setState(next);
  }, []);

  const logout = useCallback(async () => {
    try {
      await authApi.logout();
    } finally {
      setSessionToken(null);
      localStorage.removeItem(STORAGE_KEY);
      setState({ token: null, user: null });
    }
  }, []);

  const value = useMemo<AuthContextValue>(
    () => ({
      token: state.token,
      user: state.user,
      isAuthenticated: state.token !== null,
      isAdmin: state.user?.role === 'admin',
      login,
      logout,
    }),
    [state, login, logout],
  );

  return <AuthContext value={value}>{children}</AuthContext>;
}

// 读取认证上下文；必须在 AuthProvider 内使用
export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error('useAuth 必须在 AuthProvider 内使用');
  return ctx;
}
