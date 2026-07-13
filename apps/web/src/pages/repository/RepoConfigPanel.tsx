// 仓库配置面板（M1-FR-009）：只读基础信息 + 按仓库类型的可编辑字段 + 危险操作（删除仓库）。
// format 与 repo_type 创建后不可变，只读展示；保存经 PATCH 局部更新后由父级刷新详情。
import { useState, type FormEvent, type ReactNode } from 'react';
import type { components } from '@jianartifact/devmock/schema';
import {
  Alert,
  AlertDescription,
  AlertTitle,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
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
import { formatDateTime } from '../../lib/format';
import { DeleteRepoDialog } from './DeleteRepoDialog';

type RepoInfo = components['schemas']['RepoInfo'];
type UpdateRepoRequest = components['schemas']['UpdateRepoRequest'];
type Visibility = components['schemas']['Visibility'];
type OverwritePolicy = components['schemas']['OverwritePolicy'];

interface RepoConfigPanelProps {
  // 当前仓库详情（表单初值来源）
  repo: RepoInfo;
  // 保存成功回调（父级刷新详情）
  onSaved: () => void;
  // 删除成功回调（父级跳回仓库列表）
  onDeleted: () => void;
}

// 只读信息的一行「标签 - 值」
function ReadOnlyRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="text-sm">{children}</dd>
    </div>
  );
}

// 表单字段容器：标签 + 控件的统一纵向布局
function Field({
  label,
  htmlFor,
  children,
}: {
  label: string;
  htmlFor: string;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-2">
      <Label htmlFor={htmlFor}>{label}</Label>
      {children}
    </div>
  );
}

// 仓库配置面板
export function RepoConfigPanel({ repo, onSaved, onDeleted }: RepoConfigPanelProps) {
  const [visibility, setVisibility] = useState<Visibility>(repo.visibility);
  // 覆盖策略：空串表示「未设置」，保存时不提交该字段
  const [overwritePolicy, setOverwritePolicy] = useState<OverwritePolicy | ''>(
    repo.overwrite_policy ?? '',
  );
  const [upstreamUrl, setUpstreamUrl] = useState(repo.upstream_url ?? '');
  const [credentialRef, setCredentialRef] = useState(repo.upstream_credential_ref ?? '');
  const [membersText, setMembersText] = useState((repo.members ?? []).join('\n'));
  const [disabled, setDisabled] = useState(repo.disabled);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // 组装 PATCH 请求体：通用字段始终提交，类型相关字段仅在对应类型下提交
  const buildUpdateBody = (): UpdateRepoRequest => {
    const body: UpdateRepoRequest = { visibility, disabled };
    if (repo.repo_type === 'hosted' && overwritePolicy !== '') {
      body.overwrite_policy = overwritePolicy;
    }
    if (repo.repo_type === 'proxy') {
      if (upstreamUrl.trim() !== '') body.upstream_url = upstreamUrl.trim();
      if (credentialRef.trim() !== '') body.upstream_credential_ref = credentialRef.trim();
    }
    if (repo.repo_type === 'group') {
      body.members = membersText
        .split('\n')
        .map((line) => line.trim())
        .filter((line) => line !== '');
    }
    return body;
  };

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setError(null);
    setNotice(null);
    setSubmitting(true);
    try {
      await repoApi.update(repo.name, buildUpdateBody());
      setNotice('已保存');
      onSaved();
    } catch (err) {
      setError(err instanceof ApiClientError ? err.message : '保存仓库配置失败');
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="flex flex-col gap-6">
      <Card>
        <CardHeader>
          <CardTitle>仓库配置</CardTitle>
          <CardDescription>格式与类型在创建后不可更改；其余配置保存后立即生效。</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-6">
          {error ? (
            <Alert variant="destructive">
              <AlertTitle>保存失败</AlertTitle>
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          ) : null}
          {notice ? (
            <Alert>
              <AlertTitle>操作成功</AlertTitle>
              <AlertDescription>{notice}</AlertDescription>
            </Alert>
          ) : null}
          {/* 只读展示：创建后不可变的基础信息 */}
          <dl className="grid grid-cols-2 gap-4 sm:grid-cols-4">
            <ReadOnlyRow label="名称">{repo.name}</ReadOnlyRow>
            <ReadOnlyRow label="格式">{repo.format}</ReadOnlyRow>
            <ReadOnlyRow label="类型">{repo.repo_type}</ReadOnlyRow>
            <ReadOnlyRow label="创建时间">{formatDateTime(repo.created_at)}</ReadOnlyRow>
          </dl>
          <form
            className="grid max-w-3xl grid-cols-1 gap-x-6 gap-y-4 sm:grid-cols-2"
            onSubmit={handleSubmit}
          >
            <Field label="可见性" htmlFor="config-visibility">
              <Select
                value={visibility}
                onValueChange={(value) => setVisibility(value as Visibility)}
              >
                <SelectTrigger id="config-visibility" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="private">私有</SelectItem>
                  <SelectItem value="public">公开</SelectItem>
                </SelectContent>
              </Select>
            </Field>
            {repo.repo_type === 'hosted' ? (
              <Field label="覆盖策略" htmlFor="config-overwrite">
                <Select
                  value={overwritePolicy}
                  onValueChange={(value) => setOverwritePolicy(value as OverwritePolicy)}
                >
                  <SelectTrigger id="config-overwrite" className="w-full">
                    <SelectValue placeholder="未设置" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="allow">允许覆盖</SelectItem>
                    <SelectItem value="deny">禁止覆盖</SelectItem>
                  </SelectContent>
                </Select>
              </Field>
            ) : null}
            {repo.repo_type === 'proxy' ? (
              <>
                <Field label="上游地址" htmlFor="config-upstream-url">
                  <Input
                    id="config-upstream-url"
                    value={upstreamUrl}
                    placeholder="https://…"
                    onChange={(event) => setUpstreamUrl(event.target.value)}
                  />
                </Field>
                <Field label="上游凭据引用" htmlFor="config-credential-ref">
                  <Input
                    id="config-credential-ref"
                    value={credentialRef}
                    placeholder="凭据引用名（不含凭据值）"
                    onChange={(event) => setCredentialRef(event.target.value)}
                  />
                </Field>
              </>
            ) : null}
            {repo.repo_type === 'group' ? (
              <div className="sm:col-span-2">
                <Field label="成员（每行一个）" htmlFor="config-members">
                  <textarea
                    id="config-members"
                    value={membersText}
                    onChange={(event) => setMembersText(event.target.value)}
                    rows={5}
                    className="w-full rounded-md border border-input bg-transparent px-3 py-2 font-mono text-sm shadow-xs outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50"
                  />
                </Field>
              </div>
            ) : null}
            <Field label="状态" htmlFor="config-status">
              <Select
                value={disabled ? 'disabled' : 'enabled'}
                onValueChange={(value) => setDisabled(value === 'disabled')}
              >
                <SelectTrigger id="config-status" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="enabled">启用</SelectItem>
                  <SelectItem value="disabled">停用（格式端点按不存在处理）</SelectItem>
                </SelectContent>
              </Select>
            </Field>
            <div className="sm:col-span-2">
              <Button type="submit" disabled={submitting}>
                {submitting ? '保存中…' : '保存'}
              </Button>
            </div>
          </form>
        </CardContent>
      </Card>
      {/* 危险操作区：说明与按钮同行排布，删除须经确认对话框二次确认 */}
      <div className="flex flex-wrap items-center gap-4 rounded-lg border border-destructive/50 p-4">
        <div className="min-w-0 flex-1">
          <h4 className="text-sm font-semibold text-destructive">危险操作</h4>
          <p className="text-xs text-muted-foreground">
            删除仓库不可撤销；需选择模式并原样输入仓库名确认。
          </p>
        </div>
        <DeleteRepoDialog
          repoName={repo.name}
          onDeleted={onDeleted}
          trigger={<Button variant="destructive">删除仓库</Button>}
        />
      </div>
    </div>
  );
}
