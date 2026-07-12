// 组件验收站首页：共享组件库（packages/ui）的「组件 × 状态」矩阵。
// 左侧导航按四层分组列出全部收录组件，右侧展示所选组件的相关状态（正常 / 加载 / 空 /
// 错误 / 无权限 / 超长文本 / 大数据量），作为视觉回归与无障碍人工检查的稳定入口。
import { useMemo, useState, type ReactNode } from 'react';
import {
  Alert,
  AlertDescription,
  AlertTitle,
  AppShell,
  Badge,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
  cn,
  DataTable,
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
  EmptyState,
  ErrorState,
  ForbiddenState,
  Input,
  Label,
  LoadingState,
  PageHeader,
  Pagination,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Separator,
  Skeleton,
  StatusBadge,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  TableSkeleton,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
  type ColumnDef,
} from '@jianartifact/ui';

// 用于验证超长文本换行 / 截断的示例文案
const LONG_LABEL = '一个用于验证按钮在受限宽度下如何处理超长文案的很长很长的标签文本';
const LONG_TITLE =
  '超长标题：用于验证组件在受限宽度下对超长标题的换行与截断处理是否稳定不破坏整体布局';
const LONG_DESCRIPTION =
  '这是一段较长的描述文本，用来验证组件在多行文本下的排版稳定性、行高与留白是否合理，' +
  '以及是否会溢出容器边界造成横向滚动或布局抖动。';

// 分组顺序：基础 / 复合 / 状态 / 布局
const GROUPS = ['基础', '复合', '状态', '布局'] as const;
type GroupName = (typeof GROUPS)[number];

// 迁移任务十态
const TASK_STATUSES = [
  'queued',
  'running',
  'pausing',
  'paused',
  'cancelling',
  'cancelled',
  'succeeded',
  'partial',
  'failed',
  'interrupted',
];

// 表格示例行模型
interface RepoRow {
  id: number;
  name: string;
  type: string;
  visibility: string;
  status: string;
}

// 生成指定条数的示例仓库行，字段循环取值以覆盖多种徽标
function makeRows(count: number): RepoRow[] {
  const types = ['hosted', 'proxy', 'group'];
  const visibilities = ['public', 'private'];
  const statuses = ['queued', 'running', 'succeeded', 'partial', 'failed', 'interrupted'];
  return Array.from({ length: count }, (_, index) => ({
    id: index + 1,
    name: `仓库-${String(index + 1).padStart(3, '0')}`,
    type: types[index % types.length] ?? 'hosted',
    visibility: visibilities[index % visibilities.length] ?? 'public',
    status: statuses[index % statuses.length] ?? 'queued',
  }));
}

// 少量示例行（正常态）
const sampleRows = makeRows(5);

// 表格列定义：类型 / 可见性 / 任务状态列用 StatusBadge 渲染
const repoColumns: ColumnDef<RepoRow>[] = [
  { accessorKey: 'name', header: '名称' },
  {
    accessorKey: 'type',
    header: '类型',
    cell: ({ row }) => <StatusBadge kind="repo-type" value={row.original.type} />,
  },
  {
    accessorKey: 'visibility',
    header: '可见性',
    cell: ({ row }) => <StatusBadge kind="visibility" value={row.original.visibility} />,
  },
  {
    accessorKey: 'status',
    header: '任务状态',
    cell: ({ row }) => <StatusBadge kind="task-status" value={row.original.status} />,
  },
];

// 首页状态覆盖说明：组件 × 覆盖状态
const COVERAGE: Array<{ component: string; states: string }> = [
  { component: 'StatusBadge', states: '正常（可见性 / 类型 / 十态任务状态）、未知值回退' },
  { component: 'Pagination', states: '正常、首页 / 末页边界、超大数据量' },
  { component: 'DataTable', states: '正常、加载、空、大数据量分页' },
  { component: 'EmptyState', states: '空（默认 / 带操作 / 超长文本）' },
  { component: 'ErrorState', states: '错误（可重试 / 不可重试 / 超长文本）' },
  { component: 'LoadingState / TableSkeleton', states: '加载（骨架行 / 表格骨架）' },
  { component: 'ForbiddenState', states: '无权限（默认 / 自定义文案）' },
  { component: 'AppShell / PageHeader', states: '正常、无操作、超长文本' },
  { component: '基础组件', states: '正常、禁用、超长文本（按适用性）' },
];

// 单个状态展示区块：中文小标题标明「组件名 · 状态」
function MatrixBlock({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-3">
      <h3 className="text-sm font-medium text-muted-foreground">{title}</h3>
      <div className="rounded-lg border bg-card p-4">{children}</div>
    </section>
  );
}

// 组件展示页外壳：组件名 + 简介 + 若干状态区块
function ComponentPage({
  name,
  description,
  children,
}: {
  name: string;
  description?: string;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-col gap-1">
        <h2 className="text-xl font-semibold">{name}</h2>
        {description ? <p className="text-sm text-muted-foreground">{description}</p> : null}
      </div>
      {children}
    </div>
  );
}

// 导航按钮：受控高亮，选中项带 aria-current
function NavButton({
  id,
  name,
  selectedId,
  onSelect,
}: {
  id: string;
  name: string;
  selectedId: string;
  onSelect: (id: string) => void;
}) {
  const active = id === selectedId;
  return (
    <button
      type="button"
      onClick={() => onSelect(id)}
      aria-current={active ? 'page' : undefined}
      className={cn(
        'rounded-md px-2 py-1.5 text-left text-sm transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none',
        active ? 'bg-accent font-medium text-accent-foreground' : 'text-foreground',
      )}
    >
      {name}
    </button>
  );
}

// 大数据量表格：本地分页 130 行示例数据
function BigTableDemo() {
  const allRows = useMemo(() => makeRows(130), []);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(20);
  const start = (page - 1) * pageSize;
  const pageRows = allRows.slice(start, start + pageSize);
  return (
    <DataTable
      columns={repoColumns}
      data={pageRows}
      page={page}
      pageSize={pageSize}
      total={allRows.length}
      onPageChange={setPage}
      onPageSizeChange={(next) => {
        setPageSize(next);
        setPage(1);
      }}
    />
  );
}

// 可交互分页示例
function PaginationDemo() {
  const [page, setPage] = useState(1);
  return <Pagination page={page} pageSize={20} total={130} onPageChange={setPage} />;
}

// 一条导航项：id 唯一、name 为导航与索引显示名、group 所属分组、render 渲染状态矩阵
interface DemoEntry {
  id: string;
  name: string;
  group: GroupName;
  render: () => ReactNode;
}

const entries: DemoEntry[] = [
  {
    id: 'button',
    name: 'Button',
    group: '基础',
    render: () => (
      <ComponentPage name="Button 按钮" description="基础动作按钮，覆盖 variant 与禁用态">
        <MatrixBlock title="Button · 正常态（variant 矩阵）">
          <div className="flex flex-wrap gap-3">
            <Button>默认</Button>
            <Button variant="secondary">次要</Button>
            <Button variant="destructive">危险</Button>
            <Button variant="outline">描边</Button>
            <Button variant="ghost">幽灵</Button>
            <Button variant="link">链接</Button>
          </div>
        </MatrixBlock>
        <MatrixBlock title="Button · 禁用态">
          <div className="flex flex-wrap gap-3">
            <Button disabled>默认</Button>
            <Button variant="outline" disabled>
              描边
            </Button>
          </div>
        </MatrixBlock>
        <MatrixBlock title="Button · 超长文本">
          <div className="max-w-xs">
            <Button className="w-full">{LONG_LABEL}</Button>
          </div>
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'input',
    name: 'Input',
    group: '基础',
    render: () => (
      <ComponentPage name="Input 输入框" description="文本输入，覆盖正常 / 禁用 / 校验失败">
        <MatrixBlock title="Input · 正常态（含 Label）">
          <div className="flex max-w-sm flex-col gap-2">
            <Label htmlFor="wiki-repo-name">仓库名称</Label>
            <Input id="wiki-repo-name" placeholder="请输入仓库名称" />
          </div>
        </MatrixBlock>
        <MatrixBlock title="Input · 禁用态">
          <Input className="max-w-sm" disabled placeholder="不可编辑" />
        </MatrixBlock>
        <MatrixBlock title="Input · 校验失败">
          <Input className="max-w-sm" aria-invalid defaultValue="非法值" />
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'label',
    name: 'Label',
    group: '基础',
    render: () => (
      <ComponentPage name="Label 标签" description="与表单控件关联的标签">
        <MatrixBlock title="Label · 正常态">
          <div className="flex max-w-sm flex-col gap-2">
            <Label htmlFor="wiki-label-demo">用户名</Label>
            <Input id="wiki-label-demo" placeholder="与 Label 关联" />
          </div>
        </MatrixBlock>
        <MatrixBlock title="Label · 关联禁用控件">
          <div className="flex max-w-sm flex-col gap-2">
            <Label htmlFor="wiki-label-disabled">禁用字段</Label>
            <Input id="wiki-label-disabled" disabled placeholder="不可用" />
          </div>
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'badge',
    name: 'Badge',
    group: '基础',
    render: () => (
      <ComponentPage name="Badge 徽标" description="基础徽标变体">
        <MatrixBlock title="Badge · 变体">
          <div className="flex flex-wrap gap-2">
            <Badge>默认</Badge>
            <Badge variant="secondary">次要</Badge>
            <Badge variant="destructive">危险</Badge>
            <Badge variant="outline">描边</Badge>
          </div>
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'card',
    name: 'Card',
    group: '基础',
    render: () => (
      <ComponentPage name="Card 卡片" description="内容容器，覆盖正常与超长文本">
        <MatrixBlock title="Card · 正常态">
          <Card className="max-w-sm">
            <CardHeader>
              <CardTitle>仓库概览</CardTitle>
              <CardDescription>展示仓库的基础信息</CardDescription>
            </CardHeader>
            <CardContent>
              <p className="text-sm text-muted-foreground">这里是卡片正文内容。</p>
            </CardContent>
            <CardFooter>
              <Button size="sm" variant="outline">
                查看详情
              </Button>
            </CardFooter>
          </Card>
        </MatrixBlock>
        <MatrixBlock title="Card · 超长文本">
          <Card className="max-w-sm">
            <CardHeader>
              <CardTitle>{LONG_TITLE}</CardTitle>
              <CardDescription>{LONG_DESCRIPTION}</CardDescription>
            </CardHeader>
          </Card>
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'table',
    name: 'Table',
    group: '基础',
    render: () => (
      <ComponentPage name="Table 表格" description="基础表格样式元素">
        <MatrixBlock title="Table · 正常态">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead scope="col">名称</TableHead>
                <TableHead scope="col">类型</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow>
                <TableCell>仓库甲</TableCell>
                <TableCell>hosted</TableCell>
              </TableRow>
              <TableRow>
                <TableCell>仓库乙</TableCell>
                <TableCell>proxy</TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'skeleton',
    name: 'Skeleton',
    group: '基础',
    render: () => (
      <ComponentPage name="Skeleton 骨架" description="加载占位骨架块">
        <MatrixBlock title="Skeleton · 骨架块">
          <div className="flex flex-col gap-2">
            <Skeleton className="h-4 w-40" />
            <Skeleton className="h-4 w-64" />
            <Skeleton className="h-10 w-full" />
          </div>
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'alert',
    name: 'Alert',
    group: '基础',
    render: () => (
      <ComponentPage name="Alert 提示条" description="默认与危险两种提示">
        <MatrixBlock title="Alert · 默认">
          <Alert>
            <AlertTitle>提示</AlertTitle>
            <AlertDescription>这是一条普通提示信息。</AlertDescription>
          </Alert>
        </MatrixBlock>
        <MatrixBlock title="Alert · 危险">
          <Alert variant="destructive">
            <AlertTitle>操作失败</AlertTitle>
            <AlertDescription>请检查输入后重试。</AlertDescription>
          </Alert>
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'dialog',
    name: 'Dialog',
    group: '基础',
    render: () => (
      <ComponentPage name="Dialog 对话框" description="点击触发按钮打开模态对话框">
        <MatrixBlock title="Dialog · 交互（点击打开）">
          <Dialog>
            <DialogTrigger asChild>
              <Button variant="outline">打开对话框</Button>
            </DialogTrigger>
            <DialogContent>
              <DialogHeader>
                <DialogTitle>确认删除</DialogTitle>
                <DialogDescription>此操作不可撤销，请谨慎确认。</DialogDescription>
              </DialogHeader>
              <DialogFooter>
                <DialogClose asChild>
                  <Button variant="outline">取消</Button>
                </DialogClose>
                <Button variant="destructive">删除</Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'select',
    name: 'Select',
    group: '基础',
    render: () => (
      <ComponentPage name="Select 下拉选择" description="单选下拉">
        <MatrixBlock title="Select · 正常态">
          <div className="flex max-w-xs flex-col gap-2">
            <Label htmlFor="wiki-select-format">格式</Label>
            <Select>
              <SelectTrigger id="wiki-select-format" className="w-full">
                <SelectValue placeholder="选择格式" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="raw">Raw</SelectItem>
                <SelectItem value="maven">Maven</SelectItem>
                <SelectItem value="npm">npm</SelectItem>
              </SelectContent>
            </Select>
          </div>
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'tabs',
    name: 'Tabs',
    group: '基础',
    render: () => (
      <ComponentPage name="Tabs 选项卡" description="分段切换内容">
        <MatrixBlock title="Tabs · 正常态">
          <Tabs defaultValue="overview">
            <TabsList>
              <TabsTrigger value="overview">概览</TabsTrigger>
              <TabsTrigger value="files">文件</TabsTrigger>
              <TabsTrigger value="settings">设置</TabsTrigger>
            </TabsList>
            <TabsContent value="overview">概览内容</TabsContent>
            <TabsContent value="files">文件内容</TabsContent>
            <TabsContent value="settings">设置内容</TabsContent>
          </Tabs>
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'separator',
    name: 'Separator',
    group: '基础',
    render: () => (
      <ComponentPage name="Separator 分隔线" description="水平与垂直分隔">
        <MatrixBlock title="Separator · 水平 / 垂直">
          <div className="flex flex-col gap-3">
            <div>上方内容</div>
            <Separator />
            <div className="flex h-6 items-center gap-3">
              <span>左</span>
              <Separator orientation="vertical" />
              <span>右</span>
            </div>
          </div>
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'status-badge',
    name: 'StatusBadge',
    group: '复合',
    render: () => (
      <ComponentPage
        name="StatusBadge 状态徽标"
        description="可见性 / 仓库类型 / 迁移任务十态的语义色映射"
      >
        <MatrixBlock title="StatusBadge · 可见性">
          <div className="flex flex-wrap gap-2">
            <StatusBadge kind="visibility" value="public" />
            <StatusBadge kind="visibility" value="private" />
          </div>
        </MatrixBlock>
        <MatrixBlock title="StatusBadge · 仓库类型">
          <div className="flex flex-wrap gap-2">
            <StatusBadge kind="repo-type" value="hosted" />
            <StatusBadge kind="repo-type" value="proxy" />
            <StatusBadge kind="repo-type" value="group" />
          </div>
        </MatrixBlock>
        <MatrixBlock title="StatusBadge · 任务状态（十态）">
          <div className="flex flex-wrap gap-2">
            {TASK_STATUSES.map((status) => (
              <StatusBadge key={status} kind="task-status" value={status} />
            ))}
          </div>
        </MatrixBlock>
        <MatrixBlock title="StatusBadge · 未知值回退">
          <StatusBadge kind="task-status" value="mystery" />
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'pagination',
    name: 'Pagination',
    group: '复合',
    render: () => (
      <ComponentPage name="Pagination 分页" description="翻页、页码信息与每页条数选择">
        <MatrixBlock title="Pagination · 可交互（翻页）">
          <PaginationDemo />
        </MatrixBlock>
        <MatrixBlock title="Pagination · 首页（上一页禁用）">
          <Pagination page={1} pageSize={20} total={130} onPageChange={() => undefined} />
        </MatrixBlock>
        <MatrixBlock title="Pagination · 末页（下一页禁用）">
          <Pagination page={7} pageSize={20} total={130} onPageChange={() => undefined} />
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'data-table',
    name: 'DataTable',
    group: '复合',
    render: () => (
      <ComponentPage name="DataTable 数据表格" description="加载 / 空 / 有数据三态与大数据量分页">
        <MatrixBlock title="DataTable · 正常态">
          <DataTable columns={repoColumns} data={sampleRows} />
        </MatrixBlock>
        <MatrixBlock title="DataTable · 加载态">
          <DataTable columns={repoColumns} data={[]} isLoading />
        </MatrixBlock>
        <MatrixBlock title="DataTable · 空态">
          <DataTable columns={repoColumns} data={[]} />
        </MatrixBlock>
        <MatrixBlock title="DataTable · 大数据量（130 行分页）">
          <BigTableDemo />
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'empty-state',
    name: 'EmptyState',
    group: '状态',
    render: () => (
      <ComponentPage name="EmptyState 空态" description="居中卡片式空态">
        <MatrixBlock title="EmptyState · 默认">
          <EmptyState title="暂无数据" />
        </MatrixBlock>
        <MatrixBlock title="EmptyState · 带描述与操作">
          <EmptyState
            title="暂无仓库"
            description="点击下方按钮创建第一个仓库"
            action={<Button size="sm">新建仓库</Button>}
          />
        </MatrixBlock>
        <MatrixBlock title="EmptyState · 超长文本">
          <EmptyState title={LONG_TITLE} description={LONG_DESCRIPTION} />
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'error-state',
    name: 'ErrorState',
    group: '状态',
    render: () => (
      <ComponentPage name="ErrorState 错误态" description="destructive 警告样式，可选重试">
        <MatrixBlock title="ErrorState · 可重试">
          <ErrorState message="加载仓库列表失败，请检查网络后重试。" onRetry={() => undefined} />
        </MatrixBlock>
        <MatrixBlock title="ErrorState · 不可重试">
          <ErrorState title="权限不足" message="当前账号无权执行该操作。" />
        </MatrixBlock>
        <MatrixBlock title="ErrorState · 超长文本">
          <ErrorState message={LONG_DESCRIPTION} onRetry={() => undefined} />
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'loading-state',
    name: 'LoadingState',
    group: '状态',
    render: () => (
      <ComponentPage name="LoadingState / TableSkeleton 加载态" description="骨架行与表格骨架">
        <MatrixBlock title="LoadingState · 骨架行">
          <LoadingState rows={4} />
        </MatrixBlock>
        <MatrixBlock title="TableSkeleton · 表格骨架">
          <TableSkeleton rows={4} columns={4} />
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'forbidden-state',
    name: 'ForbiddenState',
    group: '状态',
    render: () => (
      <ComponentPage name="ForbiddenState 无权限态" description="无权访问占位">
        <MatrixBlock title="ForbiddenState · 默认">
          <ForbiddenState />
        </MatrixBlock>
        <MatrixBlock title="ForbiddenState · 自定义文案">
          <ForbiddenState message="该私有仓库对你不可见" />
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'app-shell',
    name: 'AppShell',
    group: '布局',
    render: () => (
      <ComponentPage name="AppShell 应用外壳" description="侧栏 + 顶栏 + 主内容（受限高度预览）">
        <MatrixBlock title="AppShell · 正常态">
          <div className="h-72 overflow-auto rounded-md border">
            <AppShell
              sidebar={
                <nav className="flex flex-col gap-1 p-4 text-sm">
                  <span className="rounded-md bg-accent px-2 py-1.5">仪表盘</span>
                  <span className="px-2 py-1.5 text-muted-foreground">仓库</span>
                  <span className="px-2 py-1.5 text-muted-foreground">用户</span>
                </nav>
              }
              header={<span className="text-sm font-medium">JianArtifact 管理端</span>}
            >
              <p className="text-sm text-muted-foreground">主内容区：放置页面标题与业务内容。</p>
            </AppShell>
          </div>
        </MatrixBlock>
      </ComponentPage>
    ),
  },
  {
    id: 'page-header',
    name: 'PageHeader',
    group: '布局',
    render: () => (
      <ComponentPage name="PageHeader 页面标题区" description="一级标题 + 描述 + 操作槽">
        <MatrixBlock title="PageHeader · 正常态">
          <PageHeader
            title="仓库列表"
            description="管理全部仓库的可见性与权限"
            actions={<Button size="sm">新建仓库</Button>}
          />
        </MatrixBlock>
        <MatrixBlock title="PageHeader · 无操作">
          <PageHeader title="用户管理" />
        </MatrixBlock>
        <MatrixBlock title="PageHeader · 超长文本">
          <PageHeader title={LONG_TITLE} description={LONG_DESCRIPTION} />
        </MatrixBlock>
      </ComponentPage>
    ),
  },
];

// 首页（总览）：一级标题 + 组件索引 + 状态覆盖说明表
function renderOverview() {
  return (
    <div className="flex flex-col gap-8">
      <PageHeader
        title="JianArtifact 组件验收站"
        description="共享组件库（packages/ui）的组件 × 状态矩阵，用于视觉回归与无障碍人工检查"
      />
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-semibold">组件索引</h2>
        <div className="grid gap-4 sm:grid-cols-2">
          {GROUPS.map((group) => (
            <div key={group} className="flex flex-col gap-1">
              <h3 className="text-sm font-medium text-muted-foreground">{group}</h3>
              <ul className="list-inside list-disc text-sm">
                {entries
                  .filter((entry) => entry.group === group)
                  .map((entry) => (
                    <li key={entry.id}>{entry.name}</li>
                  ))}
              </ul>
            </div>
          ))}
        </div>
      </section>
      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-semibold">状态覆盖说明</h2>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead scope="col">组件</TableHead>
              <TableHead scope="col">覆盖状态</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {COVERAGE.map((row) => (
              <TableRow key={row.component}>
                <TableCell className="font-medium">{row.component}</TableCell>
                <TableCell className="text-muted-foreground">{row.states}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </section>
    </div>
  );
}

export default function App() {
  const [selectedId, setSelectedId] = useState('overview');
  const activeEntry = entries.find((entry) => entry.id === selectedId);
  const activeRender =
    selectedId === 'overview' || !activeEntry ? renderOverview : activeEntry.render;

  return (
    <div className="flex min-h-svh w-full bg-background text-foreground">
      <aside aria-label="组件导航" className="w-60 shrink-0 border-r bg-card p-4">
        <p className="mb-4 px-2 text-sm font-semibold">组件目录</p>
        <nav className="flex flex-col gap-4">
          <NavButton id="overview" name="总览" selectedId={selectedId} onSelect={setSelectedId} />
          {GROUPS.map((group) => (
            <div key={group} className="flex flex-col gap-1">
              <p className="px-2 text-xs font-medium tracking-wide text-muted-foreground uppercase">
                {group}
              </p>
              {entries
                .filter((entry) => entry.group === group)
                .map((entry) => (
                  <NavButton
                    key={entry.id}
                    id={entry.id}
                    name={entry.name}
                    selectedId={selectedId}
                    onSelect={setSelectedId}
                  />
                ))}
            </div>
          ))}
        </nav>
      </aside>
      <main className="min-w-0 flex-1 p-8">
        <div className="mx-auto flex max-w-4xl flex-col gap-8">{activeRender()}</div>
      </main>
    </div>
  );
}
