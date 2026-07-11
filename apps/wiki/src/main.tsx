// 组件验收站入口：加载全局样式并把根组件挂载到 #root
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import './index.css';

// 挂载点由 index.html 静态提供，缺失说明页面骨架被破坏
const container = document.getElementById('root');
if (!container) {
  throw new Error('找不到 #root 挂载点');
}

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
