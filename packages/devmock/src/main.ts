// 浏览器演示入口：启动 mock worker，就绪后在页面上给出提示
import { worker } from './browser';

// 未匹配的请求直接放行，避免干扰页面自身资源加载
await worker.start({ onUnhandledRequest: 'bypass' });

const app = document.getElementById('app');
if (app) {
  app.textContent = 'devmock 浏览器端已就绪';
}
