// E2E：仓库详情三标签页（浏览校验和/下载、配置保存、权限增删）与列表页删除流程（FR-008/009/019）。
import { expect, test } from '@playwright/test';
import { login } from './helpers';

// 每用例先清空 localStorage/sessionStorage：隔离会话与 devmock 持久化状态
test.beforeEach(async ({ page }) => {
  await page.goto('/login');
  await page.evaluate(() => {
    localStorage.clear();
    sessionStorage.clear();
  });
});

test('详情页三标签页：浏览校验和与下载、配置保存、权限授权增删', async ({ page }) => {
  await login(page);
  await page.goto('/repos/raw-hosted');
  // 浏览：树内展开 docs 并选中 manual.pdf，出现校验和卡与下载按钮
  await page.getByRole('tree').getByRole('button', { name: 'docs' }).click();
  await page.getByRole('tree').getByRole('button', { name: 'manual.pdf' }).click();
  await expect(page.getByText('校验和')).toBeVisible();
  await expect(page.getByText('SHA-512')).toBeVisible();
  await expect(page.getByRole('link', { name: '下载' })).toHaveAttribute(
    'href',
    '/repository/raw-hosted/docs/manual.pdf',
  );
  // 配置：改可见性为私有并保存
  await page.getByRole('tab', { name: '配置' }).click();
  await page.getByRole('combobox', { name: '可见性' }).click();
  await page.getByRole('option', { name: '私有' }).click();
  await page.getByRole('button', { name: '保存' }).click();
  await expect(page.getByText('已保存')).toBeVisible();
  // 权限：添加 dev 只读授权再移除
  await page.getByRole('tab', { name: '权限' }).click();
  await page.getByRole('textbox', { name: '用户名' }).fill('dev');
  await page.getByRole('button', { name: '添加' }).click();
  await expect(page.getByRole('cell', { name: 'dev' })).toBeVisible();
  await page.getByRole('button', { name: '移除' }).click();
  await expect(page.getByRole('cell', { name: 'dev' })).toBeHidden();
});

test('列表页删除仓库：新建临时仓库后经确认对话框删除', async ({ page }) => {
  await login(page);
  await page.goto('/repos');
  // 新建临时仓库（默认 raw/hosted 即可）
  await page.getByRole('button', { name: '新建仓库' }).click();
  await page.fill('#repo-name', 'e2e-tmp-del');
  await page.getByRole('button', { name: '创建', exact: true }).click();
  await expect(page.getByRole('link', { name: 'e2e-tmp-del', exact: true })).toBeVisible();
  // 打开删除确认：原样输入仓库名后才可确认
  await page.getByRole('button', { name: '删除仓库 e2e-tmp-del' }).click();
  const confirm = page.getByRole('button', { name: '确认删除' });
  await expect(confirm).toBeDisabled();
  await page.getByLabel('输入仓库名 e2e-tmp-del 以确认').fill('e2e-tmp-del');
  await expect(confirm).toBeEnabled();
  await confirm.click();
  // 删除成功：页首提示且列表行消失
  await expect(page.getByText('已删除仓库 e2e-tmp-del')).toBeVisible();
  await expect(page.getByRole('link', { name: 'e2e-tmp-del', exact: true })).toBeHidden();
});
