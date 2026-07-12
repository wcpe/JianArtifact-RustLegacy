// 路由守卫：登录态与管理员态的三层访问控制。
// 注意：前端守卫只是体验优化（隐藏入口），权威权限判定一律以后端结果为准。
import type { ReactNode } from 'react';
import { Navigate, useLocation } from 'react-router-dom';
import { useAuth } from '../auth/AuthContext';

// 需登录：未登录跳登录页并记录来源
export function RequireAuth({ children }: { children: ReactNode }) {
  const { isAuthenticated } = useAuth();
  const location = useLocation();
  if (!isAuthenticated) {
    return <Navigate to="/login" replace state={{ from: location.pathname }} />;
  }
  return <>{children}</>;
}

// 需管理员：未登录跳登录，已登录非管理员跳无权限提示
export function RequireAdmin({ children }: { children: ReactNode }) {
  const { isAuthenticated, isAdmin } = useAuth();
  const location = useLocation();
  if (!isAuthenticated) {
    return <Navigate to="/login" replace state={{ from: location.pathname }} />;
  }
  if (!isAdmin) {
    return <Navigate to="/forbidden" replace />;
  }
  return <>{children}</>;
}
