// 管理端路由装配：公开层（登录）+ 需登录层（仪表盘/仓库/Token）+ 需管理员层（用户/迁移）。
// 三层守卫见 routes/guards；权威权限判定以后端为准，守卫仅隐藏入口。
import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom';
import { ForbiddenState } from '@jianartifact/ui';
import { AuthProvider } from './auth/AuthContext';
import { AuthenticatedLayout } from './routes/AuthenticatedLayout';
import { RefreshProvider } from './routes/RefreshContext';
import { RequireAdmin, RequireAuth } from './routes/guards';
import { LoginPage } from './pages/LoginPage';
import { DashboardPage } from './pages/DashboardPage';
import { RepositoriesPage } from './pages/RepositoriesPage';
import { RepositoryDetailPage } from './pages/RepositoryDetailPage';
import { TokensPage } from './pages/TokensPage';
import { UsersPage } from './pages/UsersPage';
import { MigrationPage } from './pages/MigrationPage';
import { SetupGate } from './setup/SetupGate';

interface AppProps {
  setupToken?: string | null;
}

export default function App({ setupToken = null }: AppProps) {
  return (
    <BrowserRouter>
      <SetupGate setupToken={setupToken}>
        <AuthProvider>
          <Routes>
            <Route path="/login" element={<LoginPage />} />
            <Route
              path="/forbidden"
              element={
                <div className="flex min-h-svh items-center justify-center p-8">
                  <ForbiddenState message="无权访问该页面，请联系管理员。" />
                </div>
              }
            />
            <Route
              element={
                <RequireAuth>
                  <RefreshProvider>
                    <AuthenticatedLayout />
                  </RefreshProvider>
                </RequireAuth>
              }
            >
              <Route path="/dashboard" element={<DashboardPage />} />
              <Route path="/repos" element={<RepositoriesPage />} />
              <Route path="/repos/:name" element={<RepositoryDetailPage />} />
              <Route path="/tokens" element={<TokensPage />} />
              <Route
                path="/users"
                element={
                  <RequireAdmin>
                    <UsersPage />
                  </RequireAdmin>
                }
              />
              <Route
                path="/migration"
                element={
                  <RequireAdmin>
                    <MigrationPage />
                  </RequireAdmin>
                }
              />
            </Route>
            <Route path="/" element={<Navigate to="/dashboard" replace />} />
            <Route path="*" element={<Navigate to="/dashboard" replace />} />
          </Routes>
        </AuthProvider>
      </SetupGate>
    </BrowserRouter>
  );
}
