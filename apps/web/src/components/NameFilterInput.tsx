// 名称筛选输入框：受控输入 + 300ms 防抖，用户停止输入后回调防抖值。
// 供仓库 / 用户 / Token 列表页复用；筛选经后端 q 参数做名称子串匹配，前端不整页过滤。
import { useEffect, useState } from 'react';
import { Search } from 'lucide-react';
import { Input } from '@jianartifact/ui';
import { useDebounced } from '../lib/useDebounced';

// 防抖时延（毫秒）：平衡输入响应与请求频率
const DEBOUNCE_MS = 300;

interface NameFilterInputProps {
  // 防抖后的筛选值回调（用户停止输入 300ms 后触发）
  onDebouncedChange: (value: string) => void;
  // 占位文案，缺省“按名称筛选…”
  placeholder?: string;
}

export function NameFilterInput({
  onDebouncedChange,
  placeholder = '按名称筛选…',
}: NameFilterInputProps) {
  const [value, setValue] = useState('');
  const debounced = useDebounced(value, DEBOUNCE_MS);

  // 防抖值变化时上报父级（父级据此传 q 并重置分页到第 1 页）
  useEffect(() => {
    onDebouncedChange(debounced);
  }, [debounced, onDebouncedChange]);

  return (
    <div className="relative max-w-xs">
      <Search
        className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
        aria-hidden
      />
      <Input
        type="search"
        value={value}
        onChange={(event) => setValue(event.target.value)}
        placeholder={placeholder}
        aria-label="按名称筛选"
        className="pl-8"
      />
    </div>
  );
}
