// 分页控件：上一页/下一页 + 页码信息 + 每页条数选择
import { ChevronLeftIcon, ChevronRightIcon } from 'lucide-react';

import { Button } from '../ui/button';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../ui/select';

// 每页条数默认候选项；上限 200 对齐 NFR-16
const DEFAULT_PAGE_SIZE_OPTIONS = [20, 50, 100, 200];

export interface PaginationProps {
  // 当前页码，从 1 开始
  page: number;
  // 每页条数
  pageSize: number;
  // 数据总条数
  total: number;
  // 翻页回调
  onPageChange: (page: number) => void;
  // 每页条数变更回调：提供时每页条数 select 生效（select 需回调驱动，故设为可选补充项）
  onPageSizeChange?: (pageSize: number) => void;
  // 每页条数候选项，默认 [20, 50, 100, 200]
  pageSizeOptions?: number[];
}

// 分页控件
export function Pagination({
  page,
  pageSize,
  total,
  onPageChange,
  onPageSizeChange,
  pageSizeOptions = DEFAULT_PAGE_SIZE_OPTIONS,
}: PaginationProps) {
  // 至少 1 页，避免总数为 0 时算出 0 页
  const totalPages = Math.max(1, Math.ceil(total / pageSize));
  const isFirst = page <= 1;
  const isLast = page >= totalPages;
  const pageInfo = `第 ${page} / ${totalPages} 页，共 ${total} 条`;

  return (
    <nav aria-label="分页" className="flex flex-wrap items-center justify-between gap-4">
      <div className="flex items-center gap-2 text-sm text-muted-foreground">
        <span id="pagination-page-size-label">每页</span>
        <Select value={String(pageSize)} onValueChange={(next) => onPageSizeChange?.(Number(next))}>
          <SelectTrigger
            size="sm"
            aria-labelledby="pagination-page-size-label"
            className="w-[92px]"
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {pageSizeOptions.map((option) => (
              <SelectItem key={option} value={String(option)}>
                {option} 条
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className="flex items-center gap-3">
        <span aria-live="polite" className="text-sm text-muted-foreground">
          {pageInfo}
        </span>
        <div className="flex items-center gap-1">
          <Button
            variant="outline"
            size="sm"
            disabled={isFirst}
            onClick={() => onPageChange(page - 1)}
          >
            <ChevronLeftIcon />
            上一页
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={isLast}
            onClick={() => onPageChange(page + 1)}
          >
            下一页
            <ChevronRightIcon />
          </Button>
        </div>
      </div>
    </nav>
  );
}
