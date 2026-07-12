// 登录后布局：AppShell 侧栏导航 + 顶栏（当前用户与退出）+ 主内容出口。
// 导航项按角色显隐（仅体验优化，后端仍是权威边界）。
import { Boxes, KeyRound, LayoutDashboard, LogOut, Users, Workflow } from 'lucide-react';
import { NavLink, Outlet, useNavigate } from 'react-router-dom';
import { AppShell, Button, cn } from '@jianartifact/ui';
import { useAuth } from '../auth/AuthContext';

// 单个导航项定义
interface NavItem {
  to: string;
  label: string;
  icon: typeof Boxes;
  adminOnly?: boolean;
}

const NAV_ITEMS: NavItem[] = [
  { to: '/dashboard', label: '仪表盘', icon: LayoutDashboard },
  { to: '/repos', label: '仓库', icon: Boxes },
  { to: '/tokens', label: 'API Token', icon: KeyRound },
  { to: '/users', label: '用户管理', icon: Users, adminOnly: true },
  { to: '/migration', label: 'Nexus 迁移', icon: Workflow, adminOnly: true },
];

export function AuthenticatedLayout() {
  const { user, isAdmin, logout } = useAuth();
  const navigate = useNavigate();

  const handleLogout = async () => {
    await logout();
    navigate('/login', { replace: true });
  };

  const items = NAV_ITEMS.filter((item) => !item.adminOnly || isAdmin);

  const sidebar = (
    <nav aria-label="主导航" className="flex flex-col gap-1 p-3">
      <p className="px-3 py-2 text-sm font-semibold">JianArtifact</p>
      {items.map((item) => {
        const Icon = item.icon;
        return (
          <NavLink
            key={item.to}
            to={item.to}
            className={({ isActive }) =>
              cn(
                'flex items-center gap-2 rounded-md px-3 py-2 text-sm transition-colors',
                isActive
                  ? 'bg-accent text-accent-foreground'
                  : 'text-muted-foreground hover:bg-accent hover:text-accent-foreground',
              )
            }
          >
            <Icon className="size-4" aria-hidden />
            {item.label}
          </NavLink>
        );
      })}
    </nav>
  );

  const header = (
    <div className="flex w-full items-center justify-end gap-3 px-4">
      <span className="text-sm text-muted-foreground">
        {user?.username}
        {isAdmin ? '（管理员）' : ''}
      </span>
      <Button variant="outline" size="sm" onClick={handleLogout}>
        <LogOut className="size-4" aria-hidden />
        退出
      </Button>
    </div>
  );

  return (
    <AppShell sidebar={sidebar} header={header}>
      <Outlet />
    </AppShell>
  );
}
