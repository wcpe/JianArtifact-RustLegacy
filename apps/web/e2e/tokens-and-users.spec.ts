// E2E：API Token 一次性明文与用户管理。对 devmock default 场景运行。
import { expect, test } from '@playwright/test';
import { login } from './helpers';

test('新建 Token 展示一次性明文', async ({ page }) => {
  await login(page);
  await page.goto('/tokens');
  await page.getByRole('button', { name: '新建 Token' }).click();
  await page.fill('#token-name', 'e2e-token');
  await page.getByRole('button', { name: '创建', exact: true }).click();
  // 明文仅此一次展示，devmock 生成 jat_mock_ 前缀
  await expect(page.getByText('请立即保存 Token 明文（仅此一次）')).toBeVisible();
  await expect(page.getByText(/jat_mock_/)).toBeVisible();
});

test('用户管理列出默认场景用户', async ({ page }) => {
  await login(page);
  await page.goto('/users');
  await expect(page.getByRole('heading', { name: '用户管理' })).toBeVisible();
  // default 场景含 admin 与 dev 两名用户
  await expect(page.getByRole('cell', { name: 'admin', exact: true })).toBeVisible();
  await expect(page.getByRole('cell', { name: 'dev', exact: true })).toBeVisible();
});

test('普通用户登录看不到用户管理入口', async ({ page }) => {
  await login(page, 'dev', 'dev123');
  // 侧栏「用户管理」是管理员入口，普通用户不渲染（仅隐藏入口，非安全边界）
  await expect(page.getByRole('link', { name: '用户管理' })).toBeHidden();
});
