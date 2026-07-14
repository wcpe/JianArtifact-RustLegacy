// 修改本人口令（M1-FR-045）：受控模态框——旧口令/新口令/确认新口令，由顶栏用户菜单触发。
// 两次新口令不一致由前端拦截；旧口令错误等以后端返回消息展示；成功后展示提示并清空表单。
import { useState, type FormEvent } from 'react';
import {
  Alert,
  AlertDescription,
  AlertTitle,
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
  Label,
} from '@jianartifact/ui';
import { ApiClientError } from '../api/client';
import { authApi } from '../api/endpoints';

// 表单的一个口令输入项（三个输入共用结构）
function PasswordField({
  id,
  label,
  value,
  autoComplete,
  onChange,
}: {
  id: string;
  label: string;
  value: string;
  autoComplete: string;
  onChange: (value: string) => void;
}) {
  return (
    <div className="flex flex-col gap-2">
      <Label htmlFor={id}>{label}</Label>
      <Input
        id={id}
        type="password"
        value={value}
        autoComplete={autoComplete}
        onChange={(event) => onChange(event.target.value)}
        required
      />
    </div>
  );
}

// 受控模态框：open / onOpenChange 由父级（顶栏用户菜单）控制
export function ChangePasswordDialog({
  open,
  onOpenChange,
  onSuccess,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSuccess?: () => void;
}) {
  const [oldPassword, setOldPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // 开关模态框：关闭即清空输入与提示
  const handleOpenChange = (next: boolean) => {
    onOpenChange(next);
    if (!next) {
      setOldPassword('');
      setNewPassword('');
      setConfirmPassword('');
      setError(null);
      setNotice(null);
    }
  };

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setError(null);
    // 前端拦截：两次新口令不一致无需请求后端
    if (newPassword !== confirmPassword) {
      setError('两次输入的新口令不一致');
      return;
    }
    setSubmitting(true);
    try {
      await authApi.changePassword({ old_password: oldPassword, new_password: newPassword });
      if (onSuccess) {
        onSuccess();
      } else {
        setOldPassword('');
        setNewPassword('');
        setConfirmPassword('');
        setNotice('修改成功，旧口令已失效，请使用新口令登录。');
      }
    } catch (err) {
      setError(err instanceof ApiClientError ? err.message : '修改口令失败');
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>修改口令</DialogTitle>
          <DialogDescription>修改成功后旧口令立即失效，请使用新口令登录。</DialogDescription>
        </DialogHeader>
        <form className="flex flex-col gap-4" onSubmit={handleSubmit}>
          {error ? (
            <Alert variant="destructive">
              <AlertTitle>修改失败</AlertTitle>
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          ) : null}
          {notice ? (
            <Alert>
              <AlertTitle>修改成功</AlertTitle>
              <AlertDescription>{notice}</AlertDescription>
            </Alert>
          ) : null}
          <PasswordField
            id="password-old"
            label="旧口令"
            value={oldPassword}
            autoComplete="section-change-password current-password"
            onChange={setOldPassword}
          />
          <PasswordField
            id="password-new"
            label="新口令"
            value={newPassword}
            autoComplete="section-change-password new-password"
            onChange={setNewPassword}
          />
          <PasswordField
            id="password-confirm"
            label="确认新口令"
            value={confirmPassword}
            autoComplete="section-change-password new-password"
            onChange={setConfirmPassword}
          />
          <DialogFooter>
            <Button type="submit" disabled={submitting || oldPassword === '' || newPassword === ''}>
              {submitting ? '提交中…' : '确认修改'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
