// E2E：登录鉴权与仓库浏览、建仓。对 devmock default 场景运行。
import { expect, test } from '@playwright/test';
import { login } from './helpers';

test('未登录访问受保护页跳转登录', async ({ page }) => {
  await page.goto('/repos');
  await expect(page.getByRole('heading', { name: '登录 JianArtifact' })).toBeVisible();
});

test('错误口令登录失败并展示后端消息', async ({ page }) => {
  await page.goto('/login');
  await page.fill('#login-username', 'admin');
  await page.fill('#login-password', '错误口令');
  await page.getByRole('button', { name: '登录' }).click();
  // 断言后端返回的具体消息，而非 Alert 固定标题——避免 mock 未生效时的假绿
  await expect(page.getByText('用户名或口令无效')).toBeVisible();
  await expect(page.getByRole('heading', { name: '仪表盘' })).toBeHidden();
});

test('登录后浏览仓库列表并进入详情', async ({ page }) => {
  await login(page);
  // 侧栏进入仓库
  await page.getByRole('link', { name: '仓库' }).click();
  await expect(page.getByRole('heading', { name: '仓库', exact: true })).toBeVisible();
  // default 场景含 raw-hosted 仓库
  const repoLink = page.getByRole('link', { name: 'raw-hosted', exact: true });
  await expect(repoLink).toBeVisible();
  await repoLink.click();
  // 详情页展示浏览标签
  await expect(page.getByRole('tab', { name: '浏览' })).toBeVisible();
});

test('管理员新建仓库后出现在列表', async ({ page }) => {
  await login(page);
  await page.goto('/repos');
  await page.getByRole('button', { name: '新建仓库' }).click();
  const name = 'e2e-raw';
  await page.fill('#repo-name', name);
  // 表单默认 raw/hosted/public 即可，直接提交
  await page.getByRole('button', { name: '创建', exact: true }).click();
  await expect(page.getByRole('link', { name, exact: true })).toBeVisible();
});
