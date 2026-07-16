// 场景标识与种子数据（M1-FR-035）：每个场景是一份可整体重建的初始状态。
// 本文件只产出数据，业务语义在 store.ts；测试凭据只存在于本包。
import type { components } from './schema';
import type { SeedData, SeedFile, SeedRepo, SeedTask, SeedToken, SeedUser } from './store';

type RepoInfo = components['schemas']['RepoInfo'];
type TaskInfo = components['schemas']['TaskInfo'];

/** 九类场景标识。 */
export type ScenarioId =
  | 'default'
  | 'empty'
  | 'setup-required'
  | 'restricted'
  | 'upstream-failure'
  | 'migration-running'
  | 'migration-partial'
  | 'large-repo'
  | 'stress';

/** 场景清单：中文标题与说明（供切换界面展示）。 */
export const SCENARIOS: Record<ScenarioId, { title: string; description: string }> = {
  default: {
    title: '默认数据',
    description: '两名用户、六个仓库与一条已完成的迁移历史，覆盖常规浏览路径',
  },
  empty: {
    title: '空实例',
    description: '仅一名管理员，无仓库无任务，模拟首次启动体验',
  },
  'setup-required': {
    title: '等待首启初始化',
    description: '用户库为空且一次性设置入口有效，用于验证首名管理员创建流程',
  },
  restricted: {
    title: '权限受限',
    description: '以普通用户 dev 登录，无权的 private 仓库被过滤，管理端点被拒绝',
  },
  'upstream-failure': {
    title: '上游失败',
    description: '所有 proxy 仓库上游故障，在线发现失败，就绪自检报未就绪',
  },
  'migration-running': {
    title: '迁移进行中',
    description: '默认数据之上有一个进行中的迁移任务，轮询详情可见进度推进',
  },
  'migration-partial': {
    title: '迁移部分失败',
    description: '默认数据之上有一个部分失败任务，报告含失败样本与不支持项',
  },
  'large-repo': {
    title: '大仓库分页',
    description: '单个 raw 仓库含 130 个文件条目，超过两个默认分页页',
  },
  stress: {
    title: '压测大数据',
    description: '100 个仓库各含上万文件、1000 个 Token、100 名用户，用于分页、筛选与性能压测',
  },
};

/** 场景标识类型守卫（切换端点入参校验用）。 */
export function isScenarioId(value: string): value is ScenarioId {
  return value in SCENARIOS;
}

// 种子时间统一取固定值，保证各场景数据可复现
const SEED_TIME = '2026-07-01T08:00:00Z';

/** 仅供 devmock 首启场景与测试使用的固定一次性令牌。 */
export const DEVMOCK_SETUP_TOKEN = 'devmock-initial-admin-token';

/** 种子管理员（测试凭据 admin/admin123，仅本包 mock 用）。 */
function adminUser(): SeedUser {
  return {
    info: { id: 1, username: 'admin', role: 'admin', enabled: true, created_at: SEED_TIME },
    password: 'admin123',
  };
}

/** 种子普通用户（测试凭据 dev/dev123，仅本包 mock 用）。 */
function devUser(): SeedUser {
  return {
    info: { id: 2, username: 'dev', role: 'user', enabled: true, created_at: SEED_TIME },
    password: 'dev123',
  };
}

/** 构造仓库信息：补齐公共缺省字段，减少重复（统计字段由 store 按文件列表实时汇总）。 */
function repoInfo(
  partial: Pick<RepoInfo, 'name' | 'format' | 'repo_type' | 'visibility'> & Partial<RepoInfo>,
): RepoInfo {
  return {
    disabled: false,
    created_at: SEED_TIME,
    members: null,
    upstream_url: null,
    upstream_credential_ref: null,
    overwrite_policy: null,
    version_policy: null,
    object_count: 0,
    total_bytes: 0,
    ...partial,
  };
}

/** 伪造指定十六进制长度的校验和（确定性、显然非真实值）。 */
function fakeHash(index: number, length: number): string {
  return index.toString(16).padStart(length, '0');
}

/** 按扩展名推内容类型（mock 展示用途，不求全）。 */
function contentTypeOf(path: string): string {
  if (path.endsWith('.pdf')) return 'application/pdf';
  if (path.endsWith('.zip')) return 'application/zip';
  if (path.endsWith('.tar.gz') || path.endsWith('.tgz')) return 'application/gzip';
  if (path.endsWith('.jar') || path.endsWith('.war')) return 'application/java-archive';
  if (path.endsWith('.pom') || path.endsWith('.xml')) return 'application/xml';
  if (path.endsWith('.json')) return 'application/json';
  return 'application/octet-stream';
}

/** 构造文件条目。 */
function file(path: string, size: number, index: number): SeedFile {
  return {
    path,
    size,
    sha256: fakeHash(index, 64),
    sha1: fakeHash(index, 40),
    md5: fakeHash(index, 32),
    sha512: fakeHash(index, 128),
    content_type: contentTypeOf(path),
    updated_at: SEED_TIME,
  };
}

/** 默认场景的六个仓库：三格式 × hosted/proxy，公私混搭。 */
function defaultRepos(options: { devReadableNpmInternal: boolean }): SeedRepo[] {
  return [
    {
      info: repoInfo({
        name: 'raw-hosted',
        format: 'raw',
        repo_type: 'hosted',
        visibility: 'public',
        overwrite_policy: 'allow',
      }),
      acl: [],
      files: [
        file('docs/manual.pdf', 1_048_576, 1),
        file('tools/cli-1.0.0.tar.gz', 5_242_880, 2),
        file('tools/cli-1.1.0.tar.gz', 5_400_000, 3),
      ],
    },
    {
      info: repoInfo({
        name: 'raw-proxy',
        format: 'raw',
        repo_type: 'proxy',
        visibility: 'public',
        upstream_url: 'https://mirror.example.com/files',
      }),
      acl: [],
      files: [file('archives/data-2026.zip', 20_971_520, 4)],
    },
    {
      info: repoInfo({
        name: 'maven-releases',
        format: 'maven',
        repo_type: 'hosted',
        visibility: 'private',
        version_policy: 'release',
        overwrite_policy: 'deny',
      }),
      acl: [{ user_id: 2, username: 'dev', permission: 'write' }],
      files: [
        file('com/example/demo/1.0.0/demo-1.0.0.jar', 350_000, 5),
        file('com/example/demo/1.0.0/demo-1.0.0.pom', 2_048, 6),
        file('com/example/demo/1.1.0/demo-1.1.0.jar', 360_000, 7),
      ],
    },
    {
      info: repoInfo({
        name: 'maven-central',
        format: 'maven',
        repo_type: 'proxy',
        visibility: 'public',
        upstream_url: 'https://repo.maven.apache.org/maven2',
      }),
      acl: [],
      files: [
        file('org/apache/commons/commons-lang3/3.14.0/commons-lang3-3.14.0.jar', 640_000, 8),
        file('org/apache/commons/commons-lang3/3.14.0/commons-lang3-3.14.0.pom', 1_536, 12),
      ],
    },
    {
      info: repoInfo({
        name: 'npm-internal',
        format: 'npm',
        repo_type: 'hosted',
        visibility: 'private',
      }),
      acl: options.devReadableNpmInternal
        ? [{ user_id: 2, username: 'dev', permission: 'read' }]
        : [],
      files: [
        file('@acme/ui/-/ui-1.0.0.tgz', 120_000, 9),
        file('@acme/ui/-/ui-1.1.0.tgz', 125_000, 10),
      ],
    },
    {
      info: repoInfo({
        name: 'npm-proxy',
        format: 'npm',
        repo_type: 'proxy',
        visibility: 'public',
        upstream_url: 'https://registry.npmjs.org',
        upstream_credential_ref: 'npm-upstream-cred',
      }),
      acl: [],
      files: [file('react/-/react-19.0.0.tgz', 310_000, 11)],
    },
  ];
}

/** 默认场景的已完成迁移任务与报告。 */
function succeededTask(): SeedTask {
  const info: TaskInfo = {
    task_id: 'task_hist_1',
    plan_id: 'plan_hist_1',
    status: 'succeeded',
    done_objects: 120,
    total_objects: 120,
    done_bytes: 734_003_200,
    total_bytes: 734_003_200,
    created_at: '2026-06-20T02:00:00Z',
    updated_at: '2026-06-20T03:10:00Z',
  };
  return {
    info,
    report: {
      task_id: info.task_id,
      status: 'succeeded',
      repositories: [
        {
          source: 'maven-releases',
          target: 'maven-releases',
          total: 120,
          done: 120,
          migrated: 118,
          skipped: 2,
          failed: 0,
          total_bytes: 734_003_200,
          done_bytes: 734_003_200,
          duration_ms: 4_200_000,
          credential_status: '已映射',
        },
      ],
      failures: [],
      failures_omitted: 0,
      unsupported: [],
      excluded: [],
    },
  };
}

/** 迁移进行中场景的活任务（进度由轮询推进）。 */
function runningTask(): SeedTask {
  return {
    info: {
      task_id: 'task_run_1',
      plan_id: 'plan_run_1',
      status: 'running',
      done_objects: 10,
      total_objects: 100,
      done_bytes: 104_857_600,
      total_bytes: 1_048_576_000,
      created_at: '2026-07-01T09:00:00Z',
      updated_at: '2026-07-01T09:05:00Z',
    },
    report: null,
  };
}

/** 迁移部分失败场景的任务：报告含两条失败样本与不支持项。 */
function partialTask(): SeedTask {
  const info: TaskInfo = {
    task_id: 'task_part_1',
    plan_id: 'plan_part_1',
    status: 'partial',
    done_objects: 96,
    total_objects: 100,
    done_bytes: 900_000_000,
    total_bytes: 1_000_000_000,
    created_at: '2026-06-28T02:00:00Z',
    updated_at: '2026-06-28T03:00:00Z',
  };
  return {
    info,
    report: {
      task_id: info.task_id,
      status: 'partial',
      repositories: [
        {
          source: 'npm-mirror',
          target: 'npm-proxy',
          total: 100,
          done: 96,
          migrated: 90,
          skipped: 6,
          failed: 4,
          total_bytes: 1_000_000_000,
          done_bytes: 900_000_000,
          duration_ms: 3_600_000,
          credential_status: '未映射',
        },
      ],
      failures: [
        {
          repo: 'npm-mirror',
          object_key: 'left-pad/-/left-pad-1.3.0.tgz',
          category: '上游拉取失败',
          message: '上游连接超时，对象未能完成迁移',
        },
        {
          repo: 'npm-mirror',
          object_key: 'lodash/-/lodash-4.17.21.tgz',
          category: '校验不一致',
          message: '源端校验和与清单不一致，已拒绝写入',
        },
      ],
      failures_omitted: 2,
      unsupported: [
        {
          name: 'docker-local',
          category: '非受支持格式',
          reason: 'Docker 格式属后续里程碑，未纳入迁移矩阵',
        },
      ],
      excluded: ['legacy-snapshots'],
    },
  };
}

/** 大仓库：130 个根级文件，覆盖 3 个默认分页页（NFR-16 默认页 50）。 */
function largeRepo(): SeedRepo {
  const files: SeedFile[] = [];
  for (let index = 1; index <= 130; index += 1) {
    files.push(file(`file-${String(index).padStart(3, '0')}.bin`, 1_024 * index, index));
  }
  return {
    info: repoInfo({
      name: 'big-raw',
      format: 'raw',
      repo_type: 'hosted',
      visibility: 'public',
    }),
    acl: [],
    files,
  };
}

// —— 压测场景规模常量（用于分页、筛选与性能压测）——
// 仓库总数
const STRESS_REPO_COUNT = 100;
// 每个仓库的文件条目数（100 仓库 × 1w = 100 万文件对象）
const STRESS_FILES_PER_REPO = 10_000;
// Token 总数（全部归属管理员，便于本人 Token 列表压测）
const STRESS_TOKEN_COUNT = 1_000;
// 普通用户数；另加 1 名管理员，合计 100 名
const STRESS_NORMAL_USER_COUNT = 99;
// raw 仓库文件归拢的目录基数（同一仓库的文件集中在 dir-(i % 该值) 目录下）
const STRESS_RAW_DIR_BASE = 50;

/** 压测仓库的格式：raw/maven/npm 三格式按仓库序号轮换。 */
function stressFormat(index: number): RepoInfo['format'] {
  const remainder = index % 3;
  if (remainder === 0) return 'raw';
  if (remainder === 1) return 'maven';
  return 'npm';
}

/** 压测仓库单个文件的路径：按格式产出层次化路径（i 为仓库序号，j 为文件序号）。 */
function stressFilePath(format: RepoInfo['format'], i: number, j: number): string {
  if (format === 'maven') return `com/example/demo${i}/1.${j}.0/demo${i}-1.${j}.0.jar`;
  if (format === 'npm') return `pkg${i}/-/pkg${i}-1.${j}.0.tgz`;
  // raw：同一仓库的文件集中在 dir-(i % STRESS_RAW_DIR_BASE) 目录下
  return `dir-${i % STRESS_RAW_DIR_BASE}/file-${j}.bin`;
}

/** 压测场景的 100 个仓库：格式/类型/可见性轮换，每仓上万文件（确定性生成）。 */
function stressRepos(): SeedRepo[] {
  const repos: SeedRepo[] = [];
  for (let i = 0; i < STRESS_REPO_COUNT; i += 1) {
    const format = stressFormat(i);
    const isProxy = i % 2 === 1;
    const name = `repo-${String(i).padStart(3, '0')}`;
    const files: SeedFile[] = [];
    for (let j = 0; j < STRESS_FILES_PER_REPO; j += 1) {
      // 校验和索引在全场景内唯一且确定，避免随机；大小随文件序号线性变化
      files.push(file(stressFilePath(format, i, j), 1_024 + j, i * STRESS_FILES_PER_REPO + j));
    }
    repos.push({
      info: repoInfo({
        name,
        format,
        repo_type: isProxy ? 'proxy' : 'hosted',
        visibility: i % 2 === 0 ? 'public' : 'private',
        // proxy 仓库配上游地址，hosted 无
        upstream_url: isProxy ? `https://upstream.example.com/${name}` : null,
      }),
      acl: [],
      files,
    });
  }
  return repos;
}

/** 压测场景的 1000 个 Token：token-0000..token-0999，全部归属管理员。 */
function stressTokens(): SeedToken[] {
  const tokens: SeedToken[] = [];
  for (let i = 0; i < STRESS_TOKEN_COUNT; i += 1) {
    tokens.push({
      info: { id: i + 1, name: `token-${String(i).padStart(4, '0')}`, created_at: SEED_TIME },
      userId: 1,
    });
  }
  return tokens;
}

/** 压测场景的 100 名用户：1 名管理员 + user-000..user-098 普通用户（口令 devpass）。 */
function stressUsers(): SeedUser[] {
  const users: SeedUser[] = [adminUser()];
  for (let i = 0; i < STRESS_NORMAL_USER_COUNT; i += 1) {
    // 管理员占用 id=1，普通用户从 id=2 起顺延
    users.push({
      info: {
        id: i + 2,
        username: `user-${String(i).padStart(3, '0')}`,
        role: 'user',
        enabled: true,
        created_at: SEED_TIME,
      },
      password: 'devpass',
    });
  }
  return users;
}

/** 压测场景种子：大规模仓库/文件/Token/用户，管理员登录以便访问全部数据。 */
function stressSeed(): SeedData {
  return {
    users: stressUsers(),
    tokens: stressTokens(),
    repos: stressRepos(),
    tasks: [],
    session: { userId: 1 },
    upstreamDown: false,
  };
}

/** default 场景的完整种子（多数场景在此基础上微调）。 */
function defaultSeed(): SeedData {
  return {
    users: [adminUser(), devUser()],
    tokens: [
      { info: { id: 1, name: 'ci 发布', created_at: SEED_TIME }, userId: 1 },
      { info: { id: 2, name: '本地调试', created_at: SEED_TIME }, userId: 2 },
    ],
    repos: defaultRepos({ devReadableNpmInternal: true }),
    tasks: [succeededTask()],
    session: { userId: 1 },
    upstreamDown: false,
  };
}

/** 生成场景初始数据：每次调用都返回全新对象，可整体重建。 */
export function seedFor(id: ScenarioId): SeedData {
  switch (id) {
    case 'default':
      return defaultSeed();
    case 'empty':
      return {
        users: [adminUser()],
        tokens: [],
        repos: [],
        tasks: [],
        session: { userId: 1 },
        upstreamDown: false,
      };
    case 'setup-required':
      return {
        users: [],
        tokens: [],
        repos: [],
        tasks: [],
        session: null,
        upstreamDown: false,
        setupToken: DEVMOCK_SETUP_TOKEN,
      };
    case 'restricted': {
      const seed = defaultSeed();
      seed.repos = defaultRepos({ devReadableNpmInternal: false });
      seed.session = { userId: 2 };
      return seed;
    }
    case 'upstream-failure': {
      const seed = defaultSeed();
      seed.upstreamDown = true;
      return seed;
    }
    case 'migration-running': {
      const seed = defaultSeed();
      seed.tasks.push(runningTask());
      return seed;
    }
    case 'migration-partial': {
      const seed = defaultSeed();
      seed.tasks.push(partialTask());
      return seed;
    }
    case 'large-repo':
      return {
        users: [adminUser()],
        tokens: [],
        repos: [largeRepo()],
        tasks: [],
        session: { userId: 1 },
        upstreamDown: false,
      };
    case 'stress':
      return stressSeed();
  }
}
