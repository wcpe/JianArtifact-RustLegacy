// 加载态占位：LoadingState 提供若干骨架行，TableSkeleton 生成表格骨架
import { Skeleton } from '../ui/skeleton';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '../ui/table';

export interface LoadingStateProps {
  // 骨架行数：默认 3
  rows?: number;
  // 无障碍提示文本：默认“加载中”
  label?: string;
}

// 通用加载态：以若干骨架行提示内容加载中
export function LoadingState({ rows = 3, label = '加载中' }: LoadingStateProps) {
  return (
    <div role="status" aria-busy="true" className="flex flex-col gap-2">
      <span className="sr-only">{label}</span>
      {Array.from({ length: rows }, (_, index) => (
        <Skeleton key={index} className="h-4 w-full" />
      ))}
    </div>
  );
}

export interface TableSkeletonProps {
  // 骨架行数：默认 5
  rows?: number;
  // 骨架列数：默认 4
  columns?: number;
  // 无障碍提示文本：默认“表格加载中”
  label?: string;
}

// 表格骨架：按行列数生成占位单元格，套用基础表格样式
export function TableSkeleton({ rows = 5, columns = 4, label = '表格加载中' }: TableSkeletonProps) {
  const columnIndexes = Array.from({ length: columns }, (_, index) => index);
  const rowIndexes = Array.from({ length: rows }, (_, index) => index);

  return (
    <div role="status" aria-busy="true">
      <span className="sr-only">{label}</span>
      <Table>
        <TableHeader>
          <TableRow>
            {columnIndexes.map((columnIndex) => (
              <TableHead key={columnIndex} scope="col">
                <Skeleton className="h-4 w-24" />
              </TableHead>
            ))}
          </TableRow>
        </TableHeader>
        <TableBody>
          {rowIndexes.map((rowIndex) => (
            <TableRow key={rowIndex}>
              {columnIndexes.map((columnIndex) => (
                <TableCell key={columnIndex}>
                  <Skeleton className="h-4 w-full" />
                </TableCell>
              ))}
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}
