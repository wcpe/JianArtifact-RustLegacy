// 复用工作区共享 ESLint 扁平配置；契约类型为生成物，不参与 lint
import config from '@jianartifact/eslint-config';

export default [...config, { ignores: ['src/schema.d.ts'] }];
