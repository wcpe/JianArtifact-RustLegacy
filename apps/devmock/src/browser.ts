// 浏览器端 mock worker：供前端子工程在开发模式下拦截请求
import { setupWorker } from 'msw/browser';
import { handlers } from './handlers';

// 携带全部契约 handlers 的浏览器 Service Worker 实例
export const worker = setupWorker(...handlers);
