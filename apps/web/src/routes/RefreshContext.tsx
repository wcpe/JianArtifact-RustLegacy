// 全局「刷新当前内容页」机制：内容页注册各自的重载函数，顶栏刷新按钮一键触发全部。
// 注册函数以 ref 存最新引用，故内容页无需保证回调稳定；无注册者时按钮为禁用态。
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from 'react';

interface RefreshContextValue {
  // 注册一个重载函数，返回注销函数
  register: (fn: () => void) => () => void;
  // 触发当前页全部已注册的重载
  refresh: () => void;
  // 刷新进行中（用于按钮 loading 反馈）
  refreshing: boolean;
  // 当前页是否有可刷新内容（无则按钮禁用）
  hasRefresh: boolean;
}

const RefreshContext = createContext<RefreshContextValue | null>(null);

export function RefreshProvider({ children }: { children: ReactNode }) {
  const fns = useRef(new Set<() => void>());
  const [count, setCount] = useState(0);
  const [refreshing, setRefreshing] = useState(false);

  const register = useCallback((fn: () => void) => {
    fns.current.add(fn);
    setCount(fns.current.size);
    return () => {
      fns.current.delete(fn);
      setCount(fns.current.size);
    };
  }, []);

  const refresh = useCallback(() => {
    if (fns.current.size === 0) return;
    setRefreshing(true);
    fns.current.forEach((fn) => fn());
    // 重载多为快速的内存 mock 请求，短暂显示 loading 反馈后复位
    window.setTimeout(() => setRefreshing(false), 600);
  }, []);

  return (
    <RefreshContext.Provider value={{ register, refresh, refreshing, hasRefresh: count > 0 }}>
      {children}
    </RefreshContext.Provider>
  );
}

// 顶栏刷新按钮消费上下文
export function useRefresh(): RefreshContextValue {
  const ctx = useContext(RefreshContext);
  if (!ctx) {
    throw new Error('useRefresh 必须在 RefreshProvider 内使用');
  }
  return ctx;
}

// 内容页注册重载函数：挂载时注册、卸载时注销；以 ref 取最新 fn，回调无需稳定引用
export function useRegisterRefresh(fn: () => void): void {
  const { register } = useRefresh();
  const ref = useRef(fn);
  ref.current = fn;
  useEffect(() => register(() => ref.current()), [register]);
}
