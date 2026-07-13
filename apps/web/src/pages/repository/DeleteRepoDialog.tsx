// 删除仓库确认对话框（M1-FR-009）：显式选择删除模式 + 原样输入仓库名才允许确认（危险操作防误触）。
// 列表页操作列与详情页配置 tab 共用；删除成功后由调用方决定刷新列表或跳回列表。
import { useState, type FormEvent, type ReactNode } from 'react';
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
  DialogTrigger,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@jianartifact/ui';
import { ApiClientError } from '../../api/client';
import { repoApi } from '../../api/endpoints';

// 删除模式：仅删配置 / 删除数据（与契约 query mode 取值一致）
type DeleteMode = 'config' | 'data';

interface DeleteRepoDialogProps {
  // 待删除的仓库名（确认输入须与其完全一致）
  repoName: string;
  // 触发器元素（列表页为图标按钮，配置 tab 为危险按钮）
  trigger: ReactNode;
  // 删除成功回调（列表页刷新并提示，详情页跳回列表）
  onDeleted: () => void;
}

// 删除仓库确认对话框
export function DeleteRepoDialog({ repoName, trigger, onDeleted }: DeleteRepoDialogProps) {
  const [open, setOpen] = useState(false);
  const [mode, setMode] = useState<DeleteMode>('config');
  const [confirmName, setConfirmName] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // 关闭时重置表单，避免下次打开残留上次输入
  const handleOpenChange = (next: boolean) => {
    setOpen(next);
    if (!next) {
      setMode('config');
      setConfirmName('');
      setError(null);
    }
  };

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setError(null);
    setSubmitting(true);
    try {
      await repoApi.remove(repoName, mode, confirmName);
      handleOpenChange(false);
      onDeleted();
    } catch (err) {
      setError(err instanceof ApiClientError ? err.message : '删除仓库失败');
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogTrigger asChild>{trigger}</DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>删除仓库 {repoName}</DialogTitle>
          <DialogDescription>
            「仅删配置」删除仓库配置与索引、保留数据（blob 变为无引用，可后续回收）；
            「删除数据」在删除配置的同时物理回收无引用 blob，不可恢复。
          </DialogDescription>
        </DialogHeader>
        <form className="flex flex-col gap-4" onSubmit={handleSubmit}>
          {error ? (
            <Alert variant="destructive">
              <AlertTitle>删除失败</AlertTitle>
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          ) : null}
          <div className="flex flex-col gap-2">
            <Label htmlFor={`delete-mode-${repoName}`}>删除模式</Label>
            <Select value={mode} onValueChange={(value) => setMode(value as DeleteMode)}>
              <SelectTrigger id={`delete-mode-${repoName}`} className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="config">仅删配置（保留数据）</SelectItem>
                <SelectItem value="data">删除数据（物理回收无引用 blob）</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor={`delete-confirm-${repoName}`}>输入仓库名 {repoName} 以确认</Label>
            <Input
              id={`delete-confirm-${repoName}`}
              value={confirmName}
              placeholder={repoName}
              onChange={(event) => setConfirmName(event.target.value)}
              autoComplete="off"
            />
          </div>
          <DialogFooter>
            <Button
              type="submit"
              variant="destructive"
              disabled={submitting || confirmName !== repoName}
            >
              {submitting ? '删除中…' : '确认删除'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
