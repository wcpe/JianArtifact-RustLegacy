import { useState, type FormEvent } from 'react';
import { useNavigate } from 'react-router-dom';
import { Alert, AlertDescription, AlertTitle, Button, Input, Label } from '@jianartifact/ui';
import { setupApi } from '../api/endpoints';
import { ApiClientError } from '../api/client';
import { AuthCardLayout } from '../components/AuthCardLayout';

interface SetupPageProps {
  setupToken: string | null;
  onSetupComplete: () => Promise<boolean>;
}

function validate(username: string, password: string, confirmation: string): string | null {
  if (username.trim() === '') return '请输入用户名';
  if (Array.from(password).length < 12) return '新口令至少需要 12 个字符';
  if (password !== confirmation) return '两次输入的口令不一致';
  return null;
}

export function SetupPage({ setupToken, onSetupComplete }: SetupPageProps) {
  const navigate = useNavigate();
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [confirmation, setConfirmation] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  if (!setupToken) {
    return (
      <AuthCardLayout
        title="初始化管理员"
        description="当前实例尚未创建管理员，请从受保护的部署环境打开初始化入口。"
      >
        <p className="text-base leading-relaxed font-medium">使用部署提供的一次性链接</p>
        <p className="mt-2 text-base leading-relaxed text-muted-foreground">
          链接仅用于首次初始化。请联系部署人员重新提供，不要在聊天或工单中传递令牌。
        </p>
      </AuthCardLayout>
    );
  }

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const validationError = validate(username, password, confirmation);
    if (validationError) {
      setError(validationError);
      return;
    }

    setError(null);
    setSubmitting(true);
    try {
      await setupApi.create({ token: setupToken, username, password });
      const ready = await onSetupComplete();
      if (!ready) {
        setError('管理员已创建，但初始化状态刷新失败，请确认状态后再登录。');
        return;
      }
      navigate('/login', { replace: true });
    } catch (err) {
      setError(err instanceof ApiClientError ? err.message : '初始化失败，请稍后重试');
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <AuthCardLayout
      title="初始化管理员"
      description="创建首名管理员。完成后请使用新账号登录管理端。"
    >
      <form className="flex flex-col gap-4" onSubmit={handleSubmit}>
        {error ? (
          <Alert variant="destructive">
            <AlertTitle>初始化失败</AlertTitle>
            <AlertDescription className="text-base">{error}</AlertDescription>
          </Alert>
        ) : null}
        <div className="flex flex-col gap-2">
          <Label className="text-base" htmlFor="setup-username">
            用户名
          </Label>
          <Input
            id="setup-username"
            className="text-base md:text-base"
            value={username}
            autoComplete="section-setup username"
            autoFocus
            onChange={(event) => setUsername(event.target.value)}
            disabled={submitting}
            required
          />
        </div>
        <div className="flex flex-col gap-2">
          <Label className="text-base" htmlFor="setup-password">
            新口令
          </Label>
          <Input
            id="setup-password"
            className="text-base md:text-base"
            type="password"
            value={password}
            autoComplete="section-setup new-password"
            onChange={(event) => setPassword(event.target.value)}
            disabled={submitting}
            aria-invalid={error === '新口令至少需要 12 个字符'}
            required
          />
          <p className="text-base leading-relaxed text-muted-foreground">至少 12 个字符。</p>
        </div>
        <div className="flex flex-col gap-2">
          <Label className="text-base" htmlFor="setup-confirm-password">
            确认口令
          </Label>
          <Input
            id="setup-confirm-password"
            className="text-base md:text-base"
            type="password"
            value={confirmation}
            autoComplete="section-setup new-password"
            onChange={(event) => setConfirmation(event.target.value)}
            disabled={submitting}
            aria-invalid={error === '两次输入的口令不一致'}
            required
          />
        </div>
        <Button type="submit" className="min-h-11 text-base" disabled={submitting}>
          {submitting ? '创建中…' : '创建管理员'}
        </Button>
      </form>
    </AuthCardLayout>
  );
}
