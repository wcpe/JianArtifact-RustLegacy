// mock 接线：仅在 VITE_MOCK 显式为 true 时动态载入 devmock worker 并启动拦截。
// 关键：import() 必须包在构建期常量守卫内，未显式开启时整个分支连同 devmock chunk
// 被静态消除，开发服务器默认直连真实后端代理。

// 运行时细粒度开关：URL ?mock=0 可在开发态临时关闭
function runtimeOptOut(): boolean {
  const url = new URLSearchParams(window.location.search);
  return url.get('mock') === '0';
}

// 启用 mock 后端（幂等，失败不阻断应用启动）
export async function enableMocking(): Promise<void> {
  // 构建期可静态判定的守卫：只有显式 true 才包含并启动 mock worker
  if (import.meta.env.VITE_MOCK === 'true') {
    if (runtimeOptOut()) return;
    // 动态导入：仅在上述守卫成立的构建中才会生成并加载该 chunk
    const { worker } = await import('@jianartifact/devmock/browser');
    await worker.start({ onUnhandledRequest: 'bypass' });
  }
}
