// 防抖 Hook：输入值停止变化 delayMs 毫秒后才更新返回值。
// 用于筛选输入，避免每次按键都触发一次请求。
import { useEffect, useState } from 'react';

export function useDebounced<T>(value: T, delayMs: number): T {
  const [debounced, setDebounced] = useState(value);

  useEffect(() => {
    // 值变化后启动定时器；delayMs 内再次变化则清除重来，只有停顿后才落定
    const timer = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(timer);
  }, [value, delayMs]);

  return debounced;
}
