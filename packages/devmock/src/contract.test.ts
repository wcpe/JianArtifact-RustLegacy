// devmock 契约双向比对（ADR-0012）：handlers 与 OpenAPI 契约端点集必须完全一致。
// 断言 A（无幻影）：每个业务 handler 都存在于契约；
// 断言 B（全量覆盖）：契约每个 operation 都有 handler，缺失即失败并列出缺失项。
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

// 归一化：OpenAPI 的 {param} 与 msw 的 :param 统一为 :param 形式
function normalizePath(path: string): string {
  return path.replace(/\{([^}]+)\}/g, ':$1');
}

// 契约中的 (method, path) 集合（归一化后）
const contractOps = new Set<string>();
for (const [path, methods] of Object.entries(doc.paths)) {
  for (const method of Object.keys(methods)) {
    contractOps.add(`${method.toUpperCase()} ${normalizePath(path)}`);
  }
}

// devmock 业务 handler 的 (method, path) 集合；/__devmock/* 是 mock 基础设施端点，
// /repository/* 是格式协议端点（独立于管理 API 契约，见 docs/API.md §2.1），均不参与比对
const handlerOps = new Set(
  handlers
    .map((h) => ({ method: h.info.method.toString().toUpperCase(), path: String(h.info.path) }))
    .filter((op) => !op.path.includes('/__devmock/') && !op.path.includes('/repository/'))
    .map((op) => `${op.method} ${op.path.replace(/^\*/, '')}`),
);

describe('devmock 契约双向比对', () => {
  it('业务 handler 不得模拟契约外端点（无幻影端点）', () => {
    const phantom = [...handlerOps].filter((op) => !contractOps.has(op));
    expect(phantom, `以下 handler 不存在于 OpenAPI 契约：${phantom.join('、')}`).toEqual([]);
  });

  it('契约每个 operation 都必须有 handler（全量覆盖）', () => {
    const missing = [...contractOps].filter((op) => !handlerOps.has(op));
    expect(missing, `以下契约 operation 缺少 handler：${missing.join('、')}`).toEqual([]);
  });
});
