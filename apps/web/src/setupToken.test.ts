import { afterEach, describe, expect, it } from 'vitest';
import { consumeSetupToken } from './setupToken';

describe('首启令牌 fragment 消费', () => {
  afterEach(() => {
    window.history.replaceState(null, '', '/');
    localStorage.clear();
    sessionStorage.clear();
  });

  it('仅解析非空 token 并立即清除 hash', () => {
    window.history.replaceState(null, '', '/setup?source=ops#token=one-time-value');

    expect(consumeSetupToken()).toBe('one-time-value');
    expect(window.location.pathname).toBe('/setup');
    expect(window.location.search).toBe('?source=ops');
    expect(window.location.hash).toBe('');
    expect(localStorage).toHaveLength(0);
    expect(sessionStorage).toHaveLength(0);
  });

  it.each(['', '#token=', '#other=value', '#token=value&other=x', '#token=%E0%A4%A'])(
    '拒绝不符合唯一格式或非法编码的 fragment：%s',
    (hash) => {
      window.history.replaceState(null, '', `/setup?source=ops${hash}`);
      expect(consumeSetupToken()).toBeNull();
      expect(window.location.pathname).toBe('/setup');
      expect(window.location.search).toBe('?source=ops');
      expect(window.location.hash).toBe('');
    },
  );

  it('安全解码 URL 编码的 token', () => {
    window.history.replaceState(null, '', '/setup#token=encoded%20value%2Bpart');

    expect(consumeSetupToken()).toBe('encoded value+part');
  });
});
