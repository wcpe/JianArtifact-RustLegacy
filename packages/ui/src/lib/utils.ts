// 样式类名合并工具：clsx 处理条件类名，tailwind-merge 消解 Tailwind 工具类冲突
import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

// 合并任意形式的类名输入，冲突的 Tailwind 工具类以后写的为准
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
