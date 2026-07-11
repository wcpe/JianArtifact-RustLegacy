// Node 端 mock 服务器工厂：供测试与本地脚本创建 MSW 拦截实例
import { setupServer } from 'msw/node';
import { handlers } from './handlers';

// 创建携带全部契约 handlers 的 Node 端拦截服务器
export function createDevmockServer() {
  return setupServer(...handlers);
}
