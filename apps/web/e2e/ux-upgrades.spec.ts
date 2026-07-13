// E2E：验收反馈驱动的 UX 改造（FR-019/034/045）——向导刷新恢复到计划步、任务暂停/继续、修改口令全流程。
import { expect, test, type Page } from '@playwright/test';
import { login } from './helpers';

// 每用例先清空 localStorage/sessionStorage：隔离向导缓存、在途任务标识与 devmock 任务持久化
test.beforeEach(async ({ page }) => {
  await page.goto('/login');
  await page.evaluate(() => {
    localStorage.clear();
    sessionStorage.clear();
  });
});

// 公共步骤：登录后把向导走到计划步（native-dir + rest 配置源）
async function goToPlanStep(page: Page): Promise<void> {
  await login(page);
  await page.goto('/migration');
  await page.fill('#source-path', '/srv/nexus/data');
  await page.fill('#source-rest-url', 'https://nexus.example.com');
  await page.getByRole('button', { name: '开始发现' }).click();
  await page.getByRole('button', { name: '生成计划' }).click();
  await expect(page.getByRole('button', { name: '开始迁移' })).toBeEnabled();
}

test('迁移向导刷新后恢复到计划步且明细表可见', async ({ page }) => {
  await goToPlanStep(page);
  // devmock 发现结果含 3 个源仓库
  await expect(page.getByText('仓库映射明细（3）')).toBeVisible();
  await page.reload();
  // 刷新后按 hash + sessionStorage 缓存恢复到计划步（不回源配置）
  await expect(page.getByRole('button', { name: '开始迁移' })).toBeVisible();
  await expect(page.getByText('仓库映射明细（3）')).toBeVisible();
  await expect(page.getByText('maven-legacy').first()).toBeVisible();
  await expect(page.locator('#source-path')).toBeHidden();
});

test('进度页点暂停变已暂停，点继续恢复运行', async ({ page }) => {
  await goToPlanStep(page);
  await page.getByRole('button', { name: '开始迁移' }).click();
  // running：暂停可用，点击后 devmock 立即转 paused
  await page.getByRole('button', { name: '暂停' }).click();
  await expect(page.getByText('已暂停')).toBeVisible();
  // paused：继续可用，点击后恢复 running
  await page.getByRole('button', { name: '继续' }).click();
  await expect(page.getByText('运行中')).toBeVisible();
});

test('修改口令后同一页面会话内用新口令重新登录', async ({ page }) => {
  await login(page);
  // 顶栏用户菜单 → 修改口令，填表提交
  await page.getByRole('button', { name: /admin/ }).click();
  await page.getByRole('menuitem', { name: '修改口令' }).click();
  await page.fill('#password-old', 'admin123');
  await page.fill('#password-new', 'e2e-new-pass');
  await page.fill('#password-confirm', 'e2e-new-pass');
  await page.getByRole('button', { name: '确认修改' }).click();
  await expect(page.getByText('修改成功', { exact: true })).toBeVisible();
  // 关闭模态框（成功后不自动关闭），再经用户菜单退出登录（SPA 导航、devmock 内存新口令仍存活）
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: /admin/ }).click();
  await page.getByRole('menuitem', { name: '退出登录' }).click();
  await expect(page.getByRole('heading', { name: '登录 JianArtifact' })).toBeVisible();
  // 同一页面会话内用新口令登录成功
  await page.fill('#login-username', 'admin');
  await page.fill('#login-password', 'e2e-new-pass');
  await page.getByRole('button', { name: '登录' }).click();
  await expect(page.getByRole('heading', { name: '仪表盘' })).toBeVisible();
});
