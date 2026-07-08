// 全局进度上下文单元测试（FR-127 / 页眉刷新基础设施）：
// 1) GlobalProgressProvider 提供的 inc/dec 正确切换 loading；
// 2) 并发 inc 后需全部 dec 才变 false；
// 3) setRouteLoading 独立控制路由维度进度；
// 4) 当前内容页刷新回调可注册、注销、执行，并避免旧注销清掉新回调。

import { useRef, useState } from 'react';
import { describe, it, expect, vi } from 'vitest';
import { render, screen, act, waitFor } from '@testing-library/react';
import { GlobalProgressProvider, useGlobalProgress } from './useGlobalProgress';

/** 探针组件：把 loading 与刷新状态暴露到 DOM 供断言。 */
function Probe({
  onRefresh = () => {},
  onAltRefresh = () => {},
}: {
  onRefresh?: () => void | Promise<void>;
  onAltRefresh?: () => void | Promise<void>;
}) {
  const {
    loading,
    inc,
    dec,
    setRouteLoading,
    hasPageRefresh,
    isPageRefreshing,
    registerPageRefreshCallback,
    clearPageRefreshCallback,
    refreshCurrentPage,
  } = useGlobalProgress();
  const firstCleanupRef = useRef<() => void>(() => {});
  const secondCleanupRef = useRef<() => void>(() => {});
  const [refreshResult, setRefreshResult] = useState('unset');

  const executeRefresh = () => {
    void refreshCurrentPage().then((result) => setRefreshResult(result ? 'true' : 'false'));
  };

  return (
    <div>
      <span data-testid="loading">{loading ? 'true' : 'false'}</span>
      <span data-testid="has-refresh">{hasPageRefresh ? 'true' : 'false'}</span>
      <span data-testid="is-refreshing">{isPageRefreshing ? 'true' : 'false'}</span>
      <span data-testid="refresh-result">{refreshResult}</span>
      <button onClick={inc} data-testid="inc">
        inc
      </button>
      <button onClick={dec} data-testid="dec">
        dec
      </button>
      <button onClick={() => setRouteLoading(true)} data-testid="route-on">
        route-on
      </button>
      <button onClick={() => setRouteLoading(false)} data-testid="route-off">
        route-off
      </button>
      <button
        onClick={() => {
          firstCleanupRef.current = registerPageRefreshCallback(onRefresh);
        }}
        data-testid="register-refresh"
      >
        register-refresh
      </button>
      <button
        onClick={() => {
          secondCleanupRef.current = registerPageRefreshCallback(onAltRefresh);
        }}
        data-testid="register-alt-refresh"
      >
        register-alt-refresh
      </button>
      <button onClick={() => firstCleanupRef.current()} data-testid="unregister-refresh">
        unregister-refresh
      </button>
      <button onClick={() => secondCleanupRef.current()} data-testid="unregister-alt-refresh">
        unregister-alt-refresh
      </button>
      <button onClick={clearPageRefreshCallback} data-testid="clear-refresh">
        clear-refresh
      </button>
      <button onClick={executeRefresh} data-testid="execute-refresh">
        execute-refresh
      </button>
    </div>
  );
}

function renderProbe(props: Parameters<typeof Probe>[0] = {}) {
  return render(
    <GlobalProgressProvider>
      <Probe {...props} />
    </GlobalProgressProvider>,
  );
}

describe('GlobalProgressProvider 请求计数维度', () => {
  it('初始 loading=false', () => {
    renderProbe();
    expect(screen.getByTestId('loading')).toHaveTextContent('false');
  });

  it('inc 后 loading=true', () => {
    renderProbe();
    act(() => {
      screen.getByTestId('inc').click();
    });
    expect(screen.getByTestId('loading')).toHaveTextContent('true');
  });

  it('inc 后 dec 回到 loading=false', () => {
    renderProbe();
    act(() => {
      screen.getByTestId('inc').click();
    });
    act(() => {
      screen.getByTestId('dec').click();
    });
    expect(screen.getByTestId('loading')).toHaveTextContent('false');
  });

  it('并发两次 inc，需两次 dec 才变 false（计数不丢失）', () => {
    renderProbe();
    act(() => {
      screen.getByTestId('inc').click();
      screen.getByTestId('inc').click();
    });
    expect(screen.getByTestId('loading')).toHaveTextContent('true');
    // 第一次 dec：计数从 2→1，仍为 true
    act(() => {
      screen.getByTestId('dec').click();
    });
    expect(screen.getByTestId('loading')).toHaveTextContent('true');
    // 第二次 dec：计数从 1→0，变 false
    act(() => {
      screen.getByTestId('dec').click();
    });
    expect(screen.getByTestId('loading')).toHaveTextContent('false');
  });

  it('dec 不会把计数降到负数（防护：超额 dec 仍为 false）', () => {
    renderProbe();
    // 从未 inc，直接 dec
    act(() => {
      screen.getByTestId('dec').click();
    });
    expect(screen.getByTestId('loading')).toHaveTextContent('false');
  });
});

describe('GlobalProgressProvider 路由切换维度', () => {
  it('setRouteLoading(true) → loading=true', () => {
    renderProbe();
    act(() => {
      screen.getByTestId('route-on').click();
    });
    expect(screen.getByTestId('loading')).toHaveTextContent('true');
  });

  it('setRouteLoading(true) 后 setRouteLoading(false) → loading=false', () => {
    renderProbe();
    act(() => {
      screen.getByTestId('route-on').click();
    });
    act(() => {
      screen.getByTestId('route-off').click();
    });
    expect(screen.getByTestId('loading')).toHaveTextContent('false');
  });
});

describe('GlobalProgressProvider 两路 OR 逻辑', () => {
  it('req loading=true 时，即使 route=false，overall=true', () => {
    renderProbe();
    act(() => {
      screen.getByTestId('inc').click();
    });
    // route 仍为 false（初始），loading 应为 true
    expect(screen.getByTestId('loading')).toHaveTextContent('true');
  });

  it('route loading=true 时，即使 req=false，overall=true', () => {
    renderProbe();
    act(() => {
      screen.getByTestId('route-on').click();
    });
    expect(screen.getByTestId('loading')).toHaveTextContent('true');
  });

  it('req=true + route=true → dec + route-off → false（两路均归零后才 false）', () => {
    renderProbe();
    act(() => {
      screen.getByTestId('inc').click();
      screen.getByTestId('route-on').click();
    });
    // 只 dec req
    act(() => {
      screen.getByTestId('dec').click();
    });
    // route 仍 true → overall still true
    expect(screen.getByTestId('loading')).toHaveTextContent('true');
    // 再 route-off
    act(() => {
      screen.getByTestId('route-off').click();
    });
    expect(screen.getByTestId('loading')).toHaveTextContent('false');
  });
});

describe('GlobalProgressProvider 当前页刷新回调', () => {
  it('未注册刷新回调时执行返回 false', async () => {
    renderProbe();

    await act(async () => {
      screen.getByTestId('execute-refresh').click();
    });

    expect(screen.getByTestId('has-refresh')).toHaveTextContent('false');
    expect(screen.getByTestId('refresh-result')).toHaveTextContent('false');
  });

  it('注册后可执行当前页刷新回调，注销后不可执行', async () => {
    const onRefresh = vi.fn();
    renderProbe({ onRefresh });

    act(() => {
      screen.getByTestId('register-refresh').click();
    });
    expect(screen.getByTestId('has-refresh')).toHaveTextContent('true');

    await act(async () => {
      screen.getByTestId('execute-refresh').click();
    });
    expect(onRefresh).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId('refresh-result')).toHaveTextContent('true');

    act(() => {
      screen.getByTestId('unregister-refresh').click();
    });
    expect(screen.getByTestId('has-refresh')).toHaveTextContent('false');
  });

  it('clearPageRefreshCallback 会清理当前页刷新回调', () => {
    renderProbe();

    act(() => {
      screen.getByTestId('register-refresh').click();
    });
    expect(screen.getByTestId('has-refresh')).toHaveTextContent('true');

    act(() => {
      screen.getByTestId('clear-refresh').click();
    });
    expect(screen.getByTestId('has-refresh')).toHaveTextContent('false');
  });

  it('旧注销函数不会清掉后来注册的新回调，避免路由切换残留误清理', async () => {
    const onRefresh = vi.fn();
    const onAltRefresh = vi.fn();
    renderProbe({ onRefresh, onAltRefresh });

    act(() => {
      screen.getByTestId('register-refresh').click();
      screen.getByTestId('register-alt-refresh').click();
      screen.getByTestId('unregister-refresh').click();
    });
    expect(screen.getByTestId('has-refresh')).toHaveTextContent('true');

    await act(async () => {
      screen.getByTestId('execute-refresh').click();
    });
    expect(onRefresh).not.toHaveBeenCalled();
    expect(onAltRefresh).toHaveBeenCalledTimes(1);
  });

  it('执行异步刷新期间标记 isPageRefreshing=true，完成后恢复 false', async () => {
    let resolveRefresh: () => void = () => {};
    const onRefresh = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveRefresh = resolve;
        }),
    );
    renderProbe({ onRefresh });

    act(() => {
      screen.getByTestId('register-refresh').click();
    });
    act(() => {
      screen.getByTestId('execute-refresh').click();
    });

    expect(screen.getByTestId('is-refreshing')).toHaveTextContent('true');

    await act(async () => {
      resolveRefresh();
    });
    await waitFor(() => expect(screen.getByTestId('is-refreshing')).toHaveTextContent('false'));
  });
});
