// 制品坐标推导（纯函数）：从文件路径按各格式的仓库布局约定推导坐标与客户端片段。
// 全部前端推导，不依赖后端契约扩展；无法可靠推导时返回 null（不显示格式卡）。

// Maven 坐标：groupId / artifactId / version / packaging。
export interface MavenCoords {
  groupId: string;
  artifactId: string;
  version: string;
  packaging: string;
}

// npm 包坐标：包名（可含 @scope）与版本。
export interface NpmCoords {
  name: string;
  version: string;
}

// 依赖声明片段：按构建工具分组的可复制代码。
export interface Snippet {
  label: string;
  content: string;
}

/**
 * 从 Maven2 布局路径推导 GAV：`<group 各段>/<artifactId>/<version>/<artifactId>-<version>[-classifier].<ext>`。
 * 约定：版本目录为倒数第二段、artifactId 为倒数第三段、其余为 groupId；文件名须以 `artifactId-version` 起头。
 * 不满足（如 maven-metadata.xml、校验和 sidecar、目录层级不足）时返回 null。
 */
export function deriveMavenCoords(path: string): MavenCoords | null {
  const segments = path.split('/').filter((s) => s.length > 0);
  // 至少需要 group(≥1) / artifactId / version / file 四段
  if (segments.length < 4) return null;
  const fileName = segments[segments.length - 1];
  const version = segments[segments.length - 2];
  const artifactId = segments[segments.length - 3];
  if (fileName === undefined || version === undefined || artifactId === undefined) return null;
  // 文件名须以 `artifactId-version` 起头，否则不是标准构件（metadata / 校验和等）
  if (!fileName.startsWith(`${artifactId}-${version}`)) return null;
  const groupId = segments.slice(0, segments.length - 3).join('.');
  if (groupId.length === 0) return null;
  const dot = fileName.lastIndexOf('.');
  const packaging = dot >= 0 ? fileName.slice(dot + 1) : 'jar';
  return { groupId, artifactId, version, packaging };
}

/**
 * 从 npm 布局路径推导包名与版本：`<包名>/-/<名称>-<版本>.tgz`（scoped 含 @scope）。
 * 约定：`/-/` 前为包名，tarball 去 `.tgz` 后须以「包名末段-」起头，其后为版本。
 * 不满足时返回 null。
 */
export function deriveNpmCoords(path: string): NpmCoords | null {
  const marker = path.indexOf('/-/');
  if (marker < 0) return null;
  const name = path.slice(0, marker);
  const tarball = path.slice(marker + 3);
  if (!tarball.endsWith('.tgz')) return null;
  const base = tarball.slice(0, -'.tgz'.length);
  const lastSegment = name.split('/').pop() ?? '';
  const prefix = `${lastSegment}-`;
  if (lastSegment.length === 0 || !base.startsWith(prefix)) return null;
  const version = base.slice(prefix.length);
  if (version.length === 0) return null;
  return { name, version };
}

/** Maven 依赖声明片段：Maven / Gradle（Groovy DSL）/ Gradle Kotlin DSL。 */
export function mavenDependencySnippets(coords: MavenCoords): Snippet[] {
  const { groupId, artifactId, version } = coords;
  return [
    {
      label: 'Maven',
      content: `<dependency>\n  <groupId>${groupId}</groupId>\n  <artifactId>${artifactId}</artifactId>\n  <version>${version}</version>\n</dependency>`,
    },
    {
      label: 'Gradle',
      content: `implementation '${groupId}:${artifactId}:${version}'`,
    },
    {
      label: 'Gradle KTS',
      content: `implementation("${groupId}:${artifactId}:${version}")`,
    },
  ];
}

/** npm 安装命令片段：npm / yarn / pnpm（registry 指向本仓库）。 */
export function npmInstallSnippets(coords: NpmCoords, registry: string): Snippet[] {
  const spec = `${coords.name}@${coords.version}`;
  return [
    { label: 'npm', content: `npm install ${spec} --registry=${registry}` },
    { label: 'yarn', content: `yarn add ${spec}` },
    { label: 'pnpm', content: `pnpm add ${spec} --registry=${registry}` },
  ];
}
