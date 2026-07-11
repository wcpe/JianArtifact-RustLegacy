// devmock 契约比对（ADR-0012）：handlers 必须与 OpenAPI 契约快照一致，防手工漂移。
// 阶段 1 硬断言「无幻影端点」（devmock 不得模拟契约中不存在的端点）；
// 「全量覆盖」断言随阶段 2 场景包落地启用（见 docs/specs/m1-frontend-foundation.md FR-035）。
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { handlers } from './handlers';

// OpenAPI 契约快照（由 make openapi 生成，质量门校验其与后端代码一致）
interface OpenApiDoc {
  paths: Record<string, Record<string, unknown>>;
}

const here = dirname(fileURLToPath(import.meta.url));
const doc = JSON.parse(readFileSync(join(here, '..', 'openapi.json'), 'utf-8')) as OpenApiDoc;

// 契约中的 (method, path) 集合
const contractOps = new Set<string>();
for (const [path, methods] of Object.entries(doc.paths)) {
  for (const method of Object.keys(methods)) {
    contractOps.add(`${method.toUpperCase()} ${path}`);
  }
}

// devmock 业务 handler 的 (method, path) 集合；/__devmock/* 是自身探测端点，不属业务契约
const handlerOps = handlers
  .map((h) => ({ method: h.info.method.toString().toUpperCase(), path: String(h.info.path) }))
  .filter((op) => !op.path.includes('/__devmock/'))
  .map((op) => `${op.method} ${op.path.replace(/^\*/, '')}`);

describe('devmock 契约比对', () => {
  it('业务 handler 不得模拟契约外端点（无幻影端点）', () => {
    for (const op of handlerOps) {
      expect(contractOps.has(op), `handler ${op} 不存在于 OpenAPI 契约`).toBe(true);
    }
  });

  it('输出契约覆盖率（全量覆盖断言随阶段 2 场景包启用）', () => {
    const covered = handlerOps.filter((op) => contractOps.has(op)).length;
    // 覆盖进度仅报告，不作为本阶段断言
    console.log(`devmock 契约覆盖：${covered}/${contractOps.size} 个 operation`);
    expect(contractOps.size).toBeGreaterThan(0);
  });
});
