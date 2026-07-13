// 迁移向导 · 步骤一：配置迁移源并发起发现。
// 三类来源：native-dir（离线直读数据目录）、online（在线 Nexus）、package（离线迁移包）。
// native-dir 的 Nexus 地址选填：填写则用账号口令经 REST 在线拉取仓库配置，留空则发现后手动补配置。
// 表单值随输入缓存供刷新/回退恢复；口令绝不写入任何缓存。
import { useEffect, useState, type FormEvent } from 'react';
import type { components } from '@jianartifact/devmock/schema';
import {
  Alert,
  AlertDescription,
  AlertTitle,
  Button,
  Card,
  CardContent,
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
import { migrationApi } from '../../api/endpoints';
import { StepLoading } from './StepLoading';
import { SOURCE_FORM_KEY, isSourceFormCache, loadCache, saveCache } from './wizardCache';

type MigrationSource = components['schemas']['MigrationSource'];
type SourceKind = components['schemas']['SourceKind'];
type DiscoveryResult = components['schemas']['DiscoveryResult'];

// 账号口令输入的统一帮助文案（M1-FR-032：口令仅传输与内存使用，不落库不进日志与报告）
const CREDENTIAL_HELP = '账号口令仅用于本次在线拉取，服务端不保存、不写入日志与迁移报告';

// Nexus 账号口令输入：online 与 native-dir（填了地址时）共用
function CredentialFields({
  idPrefix,
  username,
  password,
  onUsernameChange,
  onPasswordChange,
}: {
  idPrefix: string;
  username: string;
  password: string;
  onUsernameChange: (value: string) => void;
  onPasswordChange: (value: string) => void;
}) {
  return (
    <>
      <div className="flex flex-col gap-2">
        <Label htmlFor={`${idPrefix}-username`}>Nexus 用户名</Label>
        <Input
          id={`${idPrefix}-username`}
          value={username}
          autoComplete="off"
          onChange={(event) => onUsernameChange(event.target.value)}
        />
      </div>
      <div className="flex flex-col gap-2">
        <Label htmlFor={`${idPrefix}-password`}>Nexus 口令</Label>
        <Input
          id={`${idPrefix}-password`}
          type="password"
          value={password}
          autoComplete="new-password"
          onChange={(event) => onPasswordChange(event.target.value)}
        />
        <p className="text-xs text-muted-foreground">{CREDENTIAL_HELP}</p>
      </div>
    </>
  );
}

export function SourceStep({ onDiscovered }: { onDiscovered: (result: DiscoveryResult) => void }) {
  // 挂载时从缓存恢复表单值（步骤条回退 / 刷新场景）；口令不缓存，恢复后须重新输入
  const [cached] = useState(() => loadCache(SOURCE_FORM_KEY, isSourceFormCache));
  const [kind, setKind] = useState<SourceKind>(cached?.kind ?? 'native-dir');
  const [path, setPath] = useState(cached?.path ?? '');
  const [url, setUrl] = useState(cached?.url ?? '');
  const [username, setUsername] = useState(cached?.username ?? '');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // 表单值变化即缓存（刷新/回退时原样恢复）；口令绝不写入缓存
  useEffect(() => {
    saveCache(SOURCE_FORM_KEY, { kind, path, url, username });
  }, [kind, path, url, username]);

  // 按来源类别组装请求体：仅带该类别需要的字段；未填地址时不携带账号口令
  const buildSource = (): MigrationSource => {
    if (kind === 'online') {
      return { kind, url, username: username || null, password: password || null };
    }
    if (kind === 'package') {
      return { kind, path };
    }
    if (url) {
      return { kind, path, url, username: username || null, password: password || null };
    }
    return { kind, path };
  };

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setError(null);
    setSubmitting(true);
    try {
      const result = await migrationApi.discover(buildSource());
      onDiscovered(result);
    } catch (err) {
      setError(err instanceof ApiClientError ? err.message : '发现失败');
    } finally {
      setSubmitting(false);
    }
  };

  // 发现进行中：以处理态卡片替换表单，明确告知正在解析（native-dir 大数据目录尤其耗时）
  if (submitting) {
    return <StepLoading message="正在解析数据目录并枚举可迁对象…" />;
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>选择迁移来源</CardTitle>
      </CardHeader>
      <CardContent>
        <form className="flex max-w-lg flex-col gap-4" onSubmit={handleSubmit}>
          {error ? (
            <Alert variant="destructive">
              <AlertTitle>发现失败</AlertTitle>
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          ) : null}
          <div className="flex flex-col gap-2">
            <Label htmlFor="source-kind">来源类别</Label>
            <Select value={kind} onValueChange={(value) => setKind(value as SourceKind)}>
              <SelectTrigger id="source-kind" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="native-dir">离线数据目录</SelectItem>
                <SelectItem value="online">在线 Nexus 实例</SelectItem>
                <SelectItem value="package">离线迁移包</SelectItem>
              </SelectContent>
            </Select>
          </div>

          {kind === 'native-dir' ? (
            <>
              <div className="flex flex-col gap-2">
                <Label htmlFor="source-path">数据目录路径</Label>
                <Input
                  id="source-path"
                  value={path}
                  placeholder="Nexus 数据目录（须在白名单根内）"
                  onChange={(event) => setPath(event.target.value)}
                  required
                />
              </div>
              <div className="flex flex-col gap-2">
                <Label htmlFor="source-rest-url">Nexus 地址（选填）</Label>
                <Input
                  id="source-rest-url"
                  value={url}
                  placeholder="https://nexus.example.com"
                  onChange={(event) => setUrl(event.target.value)}
                />
                <p className="text-xs text-muted-foreground">
                  填写后用账号口令在线拉取仓库配置；留空则发现后手动补充 proxy 上游与 group 成员
                </p>
              </div>
              {url ? (
                <CredentialFields
                  idPrefix="source-rest"
                  username={username}
                  password={password}
                  onUsernameChange={setUsername}
                  onPasswordChange={setPassword}
                />
              ) : null}
            </>
          ) : null}

          {kind === 'online' ? (
            <>
              <div className="flex flex-col gap-2">
                <Label htmlFor="source-url">Nexus 实例地址</Label>
                <Input
                  id="source-url"
                  value={url}
                  placeholder="https://nexus.example.com"
                  onChange={(event) => setUrl(event.target.value)}
                  required
                />
              </div>
              <CredentialFields
                idPrefix="source-online"
                username={username}
                password={password}
                onUsernameChange={setUsername}
                onPasswordChange={setPassword}
              />
            </>
          ) : null}

          {kind === 'package' ? (
            <div className="flex flex-col gap-2">
              <Label htmlFor="source-package">迁移包路径</Label>
              <Input
                id="source-package"
                value={path}
                placeholder="离线迁移包路径（须在白名单根内）"
                onChange={(event) => setPath(event.target.value)}
                required
              />
            </div>
          ) : null}

          <Button type="submit">开始发现</Button>
        </form>
      </CardContent>
    </Card>
  );
}
