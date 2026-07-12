// 迁移向导 · 步骤一：配置迁移源并发起发现。
// 三类来源：native-dir（离线直读数据目录）、online（在线 Nexus）、package（离线迁移包）。
import { useState, type FormEvent } from 'react';
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

type MigrationSource = components['schemas']['MigrationSource'];
type SourceKind = components['schemas']['SourceKind'];
type ConfigSource = components['schemas']['ConfigSource'];
type DiscoveryResult = components['schemas']['DiscoveryResult'];

export function SourceStep({ onDiscovered }: { onDiscovered: (result: DiscoveryResult) => void }) {
  const [kind, setKind] = useState<SourceKind>('native-dir');
  const [path, setPath] = useState('');
  const [url, setUrl] = useState('');
  const [credentialRef, setCredentialRef] = useState('');
  const [configSource, setConfigSource] = useState<ConfigSource>('rest');
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // 按来源类别组装请求体：仅带该类别需要的字段
  const buildSource = (): MigrationSource => {
    if (kind === 'online') {
      return { kind, url, credential_ref: credentialRef || null };
    }
    if (kind === 'package') {
      return { kind, path };
    }
    return { kind, path, config_source: configSource };
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
                <Label htmlFor="source-config">配置来源</Label>
                <Select
                  value={configSource}
                  onValueChange={(value) => setConfigSource(value as ConfigSource)}
                >
                  <SelectTrigger id="source-config" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="rest">从 REST 读取</SelectItem>
                    <SelectItem value="manual">手动补充</SelectItem>
                  </SelectContent>
                </Select>
              </div>
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
              <div className="flex flex-col gap-2">
                <Label htmlFor="source-credential">凭据引用名（可选）</Label>
                <Input
                  id="source-credential"
                  value={credentialRef}
                  placeholder="运行时凭据引用名，不接受明文"
                  onChange={(event) => setCredentialRef(event.target.value)}
                />
              </div>
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

          <Button type="submit" disabled={submitting}>
            {submitting ? '发现中…' : '开始发现'}
          </Button>
        </form>
      </CardContent>
    </Card>
  );
}
