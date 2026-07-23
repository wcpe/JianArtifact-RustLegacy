// 计划步测试：仓库映射明细表按 mappings join 发现信息渲染，排除行标记。
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { components } from '@jianartifact/devmock/schema';
import { PlanStep } from './PlanStep';

type DiscoveredRepo = components['schemas']['DiscoveredRepo'];
type MigrationPlan = components['schemas']['MigrationPlan'];

// 发现仓库夹具：按名与是否有凭据引用构造
function repoFixture(name: string, hasCredential: boolean): DiscoveredRepo {
  return {
    name,
    format: 'maven',
    repo_type: 'hosted',
    has_credential_ref: hasCredential,
    object_count: 100,
    total_bytes: 1_048_576,
    config_complete: true,
    members: null,
    upstream_url: null,
  };
}

// 计划夹具：一条改名纳入 + 一条排除
function planFixture(): MigrationPlan {
  return {
    plan_id: 'plan_test',
    mappings: [
      { source: 'maven-legacy', target: 'maven-new', excluded: false },
      { source: 'raw-archive', target: 'raw-archive', excluded: true },
    ],
    conflicts: [],
    blocked: false,
    configuration_incomplete_count: 0,
    supported_objects_not_migrated: 0,
    unresolved_conflict_count: 0,
    object_count: 100,
    total_bytes: 1_048_576,
    estimated_disk_bytes: 1_258_291,
    disk_sufficient: true,
    unsupported: [],
  };
}

describe('计划步', () => {
  it('映射明细表渲染源/目标行与排除标记', () => {
    render(
      <PlanStep
        plan={planFixture()}
        repositories={[repoFixture('maven-legacy', true), repoFixture('raw-archive', false)]}
        onBack={() => undefined}
        onExecuted={() => undefined}
      />,
    );
    expect(screen.getByText('仓库映射明细（2）')).toBeInTheDocument();
    // 源与目标（改名后）
    expect(screen.getByText('maven-legacy')).toBeInTheDocument();
    expect(screen.getByText('maven-new')).toBeInTheDocument();
    // 凭据状态与是否排除徽标
    expect(screen.getByText('已映射')).toBeInTheDocument();
    expect(screen.getByText('未映射')).toBeInTheDocument();
    expect(screen.getByText('排除')).toBeInTheDocument();
    expect(screen.getByText('纳入')).toBeInTheDocument();
  });
});
