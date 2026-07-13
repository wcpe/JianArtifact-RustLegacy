// 制品坐标推导纯函数测试：Maven GAV 与 npm 包名/版本的路径约定推导及其边界。
import { describe, expect, it } from 'vitest';
import {
  deriveMavenCoords,
  deriveNpmCoords,
  mavenDependencySnippets,
  npmInstallSnippets,
} from './coordinates';

describe('Maven 坐标推导', () => {
  it('标准构件路径推导出 GAV 与打包类型', () => {
    const coords = deriveMavenCoords('com/example/demo/1.0.0/demo-1.0.0.jar');
    expect(coords).toEqual({
      groupId: 'com.example',
      artifactId: 'demo',
      version: '1.0.0',
      packaging: 'jar',
    });
  });

  it('多段 groupId 以点连接，pom 打包类型正确', () => {
    const coords = deriveMavenCoords(
      'org/apache/commons/commons-lang3/3.14.0/commons-lang3-3.14.0.pom',
    );
    expect(coords?.groupId).toBe('org.apache.commons');
    expect(coords?.artifactId).toBe('commons-lang3');
    expect(coords?.packaging).toBe('pom');
  });

  it('文件名与 artifactId-version 不匹配时返回 null（如 metadata）', () => {
    expect(deriveMavenCoords('com/example/demo/1.0.0/maven-metadata.xml')).toBeNull();
  });

  it('层级不足时返回 null', () => {
    expect(deriveMavenCoords('demo-1.0.0.jar')).toBeNull();
  });

  it('依赖片段覆盖 Maven / Gradle / Gradle KTS 三工具', () => {
    const coords = deriveMavenCoords('com/example/demo/1.0.0/demo-1.0.0.jar');
    expect(coords).not.toBeNull();
    if (!coords) return;
    const snippets = mavenDependencySnippets(coords);
    expect(snippets.map((s) => s.label)).toEqual(['Maven', 'Gradle', 'Gradle KTS']);
    expect(snippets[1]?.content).toBe("implementation 'com.example:demo:1.0.0'");
    expect(snippets[2]?.content).toBe('implementation("com.example:demo:1.0.0")');
  });
});

describe('npm 包坐标推导', () => {
  it('scoped 包路径推导出包名与版本', () => {
    expect(deriveNpmCoords('@acme/ui/-/ui-1.0.0.tgz')).toEqual({
      name: '@acme/ui',
      version: '1.0.0',
    });
  });

  it('非 scoped 包路径推导正确', () => {
    expect(deriveNpmCoords('react/-/react-19.0.0.tgz')).toEqual({
      name: 'react',
      version: '19.0.0',
    });
  });

  it('缺少 /-/ 分隔或非 tgz 时返回 null', () => {
    expect(deriveNpmCoords('react/index.js')).toBeNull();
    expect(deriveNpmCoords('@acme/ui/-/ui-1.0.0.json')).toBeNull();
  });

  it('安装片段覆盖 npm / yarn / pnpm 三工具并带 registry', () => {
    const coords = deriveNpmCoords('@acme/ui/-/ui-1.0.0.tgz');
    expect(coords).not.toBeNull();
    if (!coords) return;
    const snippets = npmInstallSnippets(coords, 'https://example.test/repository/npm-internal');
    expect(snippets.map((s) => s.label)).toEqual(['npm', 'yarn', 'pnpm']);
    expect(snippets[0]?.content).toContain(
      '--registry=https://example.test/repository/npm-internal',
    );
    expect(snippets[1]?.content).toBe('yarn add @acme/ui@1.0.0');
  });
});
