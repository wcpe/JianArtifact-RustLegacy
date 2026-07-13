// 共享组件库统一出口：组件源码统一归属 ui 包，消费方（web / wiki）按需具名导入。
// 分四层组织：基础组件（components/ui）、复合组件（composite）、状态占位（status）、页面布局（layout）。

// 基础组件层
export { Button, buttonVariants } from './components/ui/button';
export { Input } from './components/ui/input';
export { Label } from './components/ui/label';
export { Badge, badgeVariants } from './components/ui/badge';
export {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from './components/ui/card';
export {
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableFooter,
  TableHead,
  TableHeader,
  TableRow,
} from './components/ui/table';
export { Skeleton } from './components/ui/skeleton';
export { Alert, AlertDescription, AlertTitle } from './components/ui/alert';
export {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogOverlay,
  DialogPortal,
  DialogTitle,
  DialogTrigger,
} from './components/ui/dialog';
export {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectScrollDownButton,
  SelectScrollUpButton,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from './components/ui/select';
export { Tabs, TabsContent, TabsList, TabsTrigger, tabsListVariants } from './components/ui/tabs';
export { Separator } from './components/ui/separator';
export {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuItem,
  DropdownMenuSeparator,
} from './components/ui/dropdown-menu';

// 状态占位层
export { EmptyState } from './components/status/empty-state';
export type { EmptyStateProps } from './components/status/empty-state';
export { ErrorState } from './components/status/error-state';
export type { ErrorStateProps } from './components/status/error-state';
export { LoadingState, TableSkeleton } from './components/status/loading-state';
export type { LoadingStateProps, TableSkeletonProps } from './components/status/loading-state';
export { ForbiddenState } from './components/status/forbidden-state';
export type { ForbiddenStateProps } from './components/status/forbidden-state';

// 复合组件层
export { StatusBadge } from './components/composite/status-badge';
export type { StatusBadgeKind, StatusBadgeProps } from './components/composite/status-badge';
export { Pagination } from './components/composite/pagination';
export type { PaginationProps } from './components/composite/pagination';
export { DataTable } from './components/composite/data-table';
export type { DataTableProps } from './components/composite/data-table';
export type { ColumnDef } from '@tanstack/react-table';

// 页面布局层
export { AppShell } from './components/layout/app-shell';
export type { AppShellProps } from './components/layout/app-shell';
export { PageHeader } from './components/layout/page-header';
export type { PageHeaderProps } from './components/layout/page-header';

// 工具
export { cn } from './lib/utils';
