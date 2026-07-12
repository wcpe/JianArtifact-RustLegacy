// E2E 公共步骤：登录。开发态自动启用 devmock，每次加载 store 重建为 default 场景。
import { expect, type Page } from '@playwright/test';

// 以指定凭据登录并等待进入仪表盘（default 场景管理员 admin/admin123）
export async function login(page: Page, username = 'admin', password = 'admin123'): Promise<void> {
  await page.goto('/login');
  await page.fill('#login-username', username);
  await page.fill('#login-password', password);
  await page.getByRole('button', { name: '登录' }).click();
  await expect(page.getByRole('heading', { name: '仪表盘' })).toBeVisible();
}
