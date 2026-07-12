// DataTable 三态与分页渲染测试
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { DataTable, type ColumnDef } from './data-table';

interface Row {
  id: number;
  name: string;
}

const columns: ColumnDef<Row>[] = [
  { accessorKey: 'id', header: '编号' },
  { accessorKey: 'name', header: '名称' },
];

const data: Row[] = [
  { id: 1, name: '仓库甲' },
  { id: 2, name: '仓库乙' },
];

describe('DataTable 数据表格', () => {
  it('有数据时渲染表头与行', () => {
    render(<DataTable columns={columns} data={data} />);
    expect(screen.getByRole('columnheader', { name: '编号' })).toBeInTheDocument();
    expect(screen.getByRole('columnheader', { name: '名称' })).toBeInTheDocument();
    expect(screen.getByText('仓库甲')).toBeInTheDocument();
    expect(screen.getByText('仓库乙')).toBeInTheDocument();
  });

  it('加载态渲染表格骨架且不渲染真实数据', () => {
    render(<DataTable columns={columns} data={[]} isLoading />);
    expect(screen.getByRole('status')).toBeInTheDocument();
    expect(screen.queryByText('仓库甲')).not.toBeInTheDocument();
  });

  it('空数据渲染默认空态', () => {
    render(<DataTable columns={columns} data={[]} />);
    expect(screen.getByText('暂无数据')).toBeInTheDocument();
  });

  it('空数据可自定义空态', () => {
    render(<DataTable columns={columns} data={[]} emptyState={<div>空空如也</div>} />);
    expect(screen.getByText('空空如也')).toBeInTheDocument();
  });

  it('提供分页参数时渲染分页栏', () => {
    render(
      <DataTable
        columns={columns}
        data={data}
        page={1}
        pageSize={20}
        total={40}
        onPageChange={() => undefined}
      />,
    );
    expect(screen.getByRole('navigation', { name: '分页' })).toBeInTheDocument();
  });

  it('未提供分页参数时不渲染分页栏', () => {
    render(<DataTable columns={columns} data={data} />);
    expect(screen.queryByRole('navigation', { name: '分页' })).not.toBeInTheDocument();
  });
});
