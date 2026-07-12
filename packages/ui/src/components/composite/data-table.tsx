// 数据表格：基于 TanStack Table 的表格封装，覆盖加载 / 空 / 有数据三态，可选分页
import type { ReactNode } from 'react';
import { flexRender, getCoreRowModel, useReactTable, type ColumnDef } from '@tanstack/react-table';

import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '../ui/table';
import { EmptyState } from '../status/empty-state';
import { TableSkeleton } from '../status/loading-state';
import { Pagination } from './pagination';

// 供业务页复用列定义类型
export type { ColumnDef } from '@tanstack/react-table';

export interface DataTableProps<TData> {
  // 列定义
  columns: ColumnDef<TData>[];
  // 行数据
  data: TData[];
  // 加载中：为真时展示表格骨架
  isLoading?: boolean;
  // 空态：数据为空时展示，缺省用“暂无数据”空态
  emptyState?: ReactNode;
  // 当前页码（从 1 开始）
  page?: number;
  // 每页条数
  pageSize?: number;
  // 数据总条数
  total?: number;
  // 翻页回调：与 page/pageSize/total 一并提供时渲染分页栏
  onPageChange?: (page: number) => void;
  // 每页条数变更回调
  onPageSizeChange?: (pageSize: number) => void;
  // 每页条数候选项
  pageSizeOptions?: number[];
}

// 数据表格
export function DataTable<TData>({
  columns,
  data,
  isLoading = false,
  emptyState,
  page,
  pageSize,
  total,
  onPageChange,
  onPageSizeChange,
  pageSizeOptions,
}: DataTableProps<TData>) {
  const table = useReactTable({
    data,
    columns,
    getCoreRowModel: getCoreRowModel(),
  });

  // 加载态优先
  if (isLoading) {
    return <TableSkeleton columns={columns.length} />;
  }

  const rows = table.getRowModel().rows;

  // 无数据展示空态
  if (rows.length === 0) {
    return <>{emptyState ?? <EmptyState title="暂无数据" />}</>;
  }

  return (
    <div className="flex flex-col gap-4">
      <Table>
        <TableHeader>
          {table.getHeaderGroups().map((headerGroup) => (
            <TableRow key={headerGroup.id}>
              {headerGroup.headers.map((header) => (
                <TableHead key={header.id} scope="col">
                  {header.isPlaceholder
                    ? null
                    : flexRender(header.column.columnDef.header, header.getContext())}
                </TableHead>
              ))}
            </TableRow>
          ))}
        </TableHeader>
        <TableBody>
          {rows.map((row) => (
            <TableRow key={row.id}>
              {row.getVisibleCells().map((cell) => (
                <TableCell key={cell.id}>
                  {flexRender(cell.column.columnDef.cell, cell.getContext())}
                </TableCell>
              ))}
            </TableRow>
          ))}
        </TableBody>
      </Table>
      {/* 分页三要素与翻页回调齐备时才渲染分页栏（内联判空以让类型收窄为非空） */}
      {onPageChange !== undefined &&
      page !== undefined &&
      pageSize !== undefined &&
      total !== undefined ? (
        <Pagination
          page={page}
          pageSize={pageSize}
          total={total}
          onPageChange={onPageChange}
          onPageSizeChange={onPageSizeChange}
          pageSizeOptions={pageSizeOptions}
        />
      ) : null}
    </div>
  );
}
