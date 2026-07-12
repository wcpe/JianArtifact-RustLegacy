// 开发态 mock 接线：仅在开发模式或显式开关下动态载入 devmock worker 并启动拦截。
// 关键：import() 必须包在「构建期常量」守卫内——DEV 与 VITE_MOCK 在构建时被替换为
// 字面量，生产且未显式开启时整个分支连同 devmock chunk 被静态消除，
// 产物不含 mock 代码与测试凭据（FR-035 硬要求，验收以产物 grep 为证）。

// 运行时细粒度开关：URL ?mock=0 可在开发态临时关闭
function runtimeOptOut(): boolean {
  const url = new URLSearchParams(window.location.search);
  return url.get('mock') === '0';
}

// 启用 mock 后端（幂等，失败不阻断应用启动）
export async function enableMocking(): Promise<void> {
  // 构建期可静态判定的守卫：生产构建下该分支被死代码消除
  if (import.meta.env.DEV || import.meta.env.VITE_MOCK === 'true') {
    if (runtimeOptOut()) return;
    // 动态导入：仅在上述守卫成立的构建中才会生成并加载该 chunk
    const { worker } = await import('@jianartifact/devmock/browser');
    await worker.start({ onUnhandledRequest: 'bypass' });
  }
}
