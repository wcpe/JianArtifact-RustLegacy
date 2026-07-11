// 业务契约 handlers 随阶段 1 OpenAPI 接线，当前仅 devmock 自身探测端点。
import { http, HttpResponse } from 'msw';

// devmock 自身健康探测端点，用于确认 mock 层已生效。
// 谓词带 * 通配源站：Node 环境没有 document.baseURI，相对路径谓词不会被匹配。
export const handlers = [http.get('*/__devmock/health', () => HttpResponse.json({ status: 'ok' }))];
