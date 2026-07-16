// 登录页：卡片式用户名/口令表单。已登录直接跳转仪表盘；登录成功回到来源或仪表盘。
import { useState, type FormEvent } from 'react';
import { Navigate, useLocation, useNavigate } from 'react-router-dom';
import { Alert, AlertDescription, AlertTitle, Button, Input, Label } from '@jianartifact/ui';
import { ApiClientError } from '../api/client';
import { AuthCardLayout } from '../components/AuthCardLayout';
import { useAuth } from '../auth/AuthContext';

export function LoginPage() {
  const { isAuthenticated, isRestoring, login } = useAuth();
  const navigate = useNavigate();
  const location = useLocation();
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // 恢复已有令牌期间等待 auth/me 确认身份，避免先跳登录页
  if (isRestoring) return null;

  // 已登录用户不应停留在登录页，直接送往仪表盘
  if (isAuthenticated) {
    return <Navigate to="/dashboard" replace />;
  }

  // 登录成功后的目标路径：优先回到守卫记录的来源
  const from = (location.state as { from?: string } | null)?.from ?? '/dashboard';

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setError(null);
    setSubmitting(true);
    try {
      await login(username, password);
      navigate(from, { replace: true });
    } catch (err) {
      // 登录失败以后端返回的中文消息为准
      setError(err instanceof ApiClientError ? err.message : '登录失败，请稍后再试');
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <AuthCardLayout title="登录 JianArtifact" description="请输入用户名与口令以进入管理端">
      <form className="flex flex-col gap-4" onSubmit={handleSubmit}>
        {error ? (
          <Alert variant="destructive">
            <AlertTitle>登录失败</AlertTitle>
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        ) : null}
        <div className="flex flex-col gap-2">
          <Label htmlFor="login-username">用户名</Label>
          <Input
            id="login-username"
            value={username}
            autoComplete="username"
            onChange={(event) => setUsername(event.target.value)}
            required
          />
        </div>
        <div className="flex flex-col gap-2">
          <Label htmlFor="login-password">口令</Label>
          <Input
            id="login-password"
            type="password"
            value={password}
            autoComplete="current-password"
            onChange={(event) => setPassword(event.target.value)}
            required
          />
        </div>
        <Button type="submit" disabled={submitting}>
          {submitting ? '登录中…' : '登录'}
        </Button>
      </form>
    </AuthCardLayout>
  );
}
