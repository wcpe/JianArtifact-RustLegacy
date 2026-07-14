// 修改口令模态框测试（受控）：成功路径、旧口令错误路径与两次输入不一致拦截（M1-FR-045）。
import { fireEvent, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { loginAs, renderWithProviders, resetScenario } from '../test/utils';
import { ChangePasswordDialog } from './ChangePasswordDialog';

// 直接以打开态渲染受控模态框
function renderDialog(onSuccess?: () => void) {
  renderWithProviders(
    <ChangePasswordDialog open onOpenChange={() => undefined} onSuccess={onSuccess} />,
  );
}

// 填写三项口令输入并提交
async function submit(oldPassword: string, newPassword: string, confirm: string) {
  fireEvent.change(await screen.findByLabelText('旧口令'), { target: { value: oldPassword } });
  fireEvent.change(screen.getByLabelText('新口令'), { target: { value: newPassword } });
  fireEvent.change(screen.getByLabelText('确认新口令'), { target: { value: confirm } });
  fireEvent.click(screen.getByRole('button', { name: '确认修改' }));
}

describe('修改口令模态框', () => {
  beforeEach(async () => {
    localStorage.clear();
    await resetScenario('default');
    await loginAs('admin', 'admin123');
  });

  it('口令输入使用修改口令专用 autocomplete', async () => {
    renderDialog();
    expect(await screen.findByLabelText('旧口令')).toHaveAttribute(
      'autocomplete',
      'section-change-password current-password',
    );
    expect(screen.getByLabelText('新口令')).toHaveAttribute(
      'autocomplete',
      'section-change-password new-password',
    );
    expect(screen.getByLabelText('确认新口令')).toHaveAttribute(
      'autocomplete',
      'section-change-password new-password',
    );
  });

  it('旧口令正确时修改成功并展示成功提示', async () => {
    renderDialog();
    await submit('admin123', 'new-pass-1', 'new-pass-1');
    expect(await screen.findByText('修改成功', { exact: true })).toBeInTheDocument();
  });

  it('成功后调用会话清理回调', async () => {
    const onSuccess = vi.fn();
    renderDialog(onSuccess);
    await submit('admin123', 'new-pass-1', 'new-pass-1');
    await waitFor(() => expect(onSuccess).toHaveBeenCalledOnce());
  });

  it('旧口令错误时展示后端消息', async () => {
    renderDialog();
    await submit('wrong-old', 'new-pass-1', 'new-pass-1');
    expect(await screen.findByText('旧口令错误')).toBeInTheDocument();
  });

  it('两次新口令不一致时前端拦截', async () => {
    renderDialog();
    await submit('admin123', 'new-pass-1', 'new-pass-2');
    expect(await screen.findByText('两次输入的新口令不一致')).toBeInTheDocument();
  });
});
