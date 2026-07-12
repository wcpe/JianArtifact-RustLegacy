// 复用工作区共享 ESLint 扁平配置；忽略 Playwright 运行产物目录
import config from '@jianartifact/eslint-config';

export default [...config, { ignores: ['test-results/**', 'playwright-report/**'] }];
