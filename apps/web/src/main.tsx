// 管理端入口：开发态先启用 mock 后端，再挂载根组件。
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import { enableMocking } from './mock/enable';
import { consumeSetupToken } from './setupToken';
import './index.css';

// 挂载点由 index.html 静态提供，缺失说明页面骨架被破坏
const container = document.getElementById('root');
if (!container) {
  throw new Error('找不到 #root 挂载点');
}

// 在 StrictMode 外只消费一次 fragment，令牌仅通过组件属性保留在内存中
const setupToken = consumeSetupToken();

// 先启用 mock（生产构建下为空操作），再渲染，避免首屏请求早于拦截建立
enableMocking()
  .catch(() => {
    // mock 启用失败不阻断应用（如生产环境无 worker）
  })
  .finally(() => {
    createRoot(container).render(
      <StrictMode>
        <App setupToken={setupToken} />
      </StrictMode>,
    );
  });
