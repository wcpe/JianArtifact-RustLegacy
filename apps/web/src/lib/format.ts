// 展示层通用格式化：字节数与日期时间，供各页面复用避免重复实现。

// 字节数候选单位（十进制近似，逐级除以 1024）
const BYTE_UNITS = ['KB', 'MB', 'GB', 'TB', 'PB'];

// 人类可读字节数：小于 1024 直接标 B，否则逐级换算并保留一位小数
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes) || bytes < 0) return '—';
  if (bytes < 1024) return `${bytes} B`;
  let value = bytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < BYTE_UNITS.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  return `${value.toFixed(1)} ${BYTE_UNITS[unitIndex] ?? 'PB'}`;
}

// 本地化日期时间；空值回退占位符，非法值原样返回
export function formatDateTime(value: string | null | undefined): string {
  if (!value) return '—';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString();
}
