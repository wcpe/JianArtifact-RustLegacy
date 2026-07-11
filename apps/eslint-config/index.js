// 工作区共享 ESLint 扁平配置：JS 推荐规则 + TypeScript 严格规则 + React Hooks 规则，
// 末尾以 Prettier 兼容配置关闭与格式化冲突的规则。
import js from '@eslint/js';
import tseslint from 'typescript-eslint';
import reactHooks from 'eslint-plugin-react-hooks';
import prettier from 'eslint-config-prettier';

export default tseslint.config(
  js.configs.recommended,
  ...tseslint.configs.strict,
  {
    plugins: { 'react-hooks': reactHooks },
    rules: reactHooks.configs.recommended.rules,
  },
  prettier,
  { ignores: ['dist/**'] },
);
