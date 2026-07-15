//! 仓库领域类型与无 IO 授权判定。

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::domain::Role;

/// 制品格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// 任意路径直传。
    Raw,
    /// Maven2 布局。
    Maven,
    /// npm registry 协议。
    Npm,
}

/// 仓库类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RepoType {
    /// 托管仓库。
    Hosted,
    /// 代理仓库。
    Proxy,
    /// 只读聚合仓库。
    Group,
}

/// 仓库可见性。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    /// 匿名可读。
    Public,
    /// 仅授权用户可读。
    Private,
}

/// 仓库 ACL 动作，权限向下包含。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    /// 下载、浏览与读取元数据。
    Read,
    /// 上传、覆盖与删除制品。
    Write,
    /// 管理仓库配置与 ACL。
    Admin,
}

/// hosted 同路径覆盖策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OverwritePolicy {
    /// 允许覆盖。
    Allow,
    /// 拒绝覆盖。
    Deny,
}

/// Maven hosted 版本策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum VersionPolicy {
    /// 仅 release。
    Release,
    /// 仅 snapshot。
    Snapshot,
    /// release 与 snapshot 混合。
    Mixed,
}

macro_rules! impl_db_enum {
    ($type:ty, { $($variant:path => $value:literal),+ $(,)? }) => {
        impl $type {
            pub(crate) fn as_db_str(self) -> &'static str {
                match self {
                    $($variant => $value),+
                }
            }

            pub(crate) fn from_db_str(value: &str) -> Option<Self> {
                match value {
                    $($value => Some($variant)),+,
                    _ => None,
                }
            }
        }
    };
}

impl_db_enum!(Format, {
    Format::Raw => "raw",
    Format::Maven => "maven",
    Format::Npm => "npm",
});
impl_db_enum!(RepoType, {
    RepoType::Hosted => "hosted",
    RepoType::Proxy => "proxy",
    RepoType::Group => "group",
});
impl_db_enum!(Visibility, {
    Visibility::Public => "public",
    Visibility::Private => "private",
});
impl_db_enum!(Permission, {
    Permission::Read => "read",
    Permission::Write => "write",
    Permission::Admin => "admin",
});
impl_db_enum!(OverwritePolicy, {
    OverwritePolicy::Allow => "allow",
    OverwritePolicy::Deny => "deny",
});
impl_db_enum!(VersionPolicy, {
    VersionPolicy::Release => "release",
    VersionPolicy::Snapshot => "snapshot",
    VersionPolicy::Mixed => "mixed",
});

/// 授权主体。无效凭据与被禁用身份显式进入纯函数，确保统一返回 401。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    /// 未携带身份凭据。
    Anonymous,
    /// 已解析身份；enabled 为当前数据库中的即时状态。
    Authenticated {
        /// 用户标识。
        user_id: i64,
        /// 全局角色。
        role: Role,
        /// 账户是否启用。
        enabled: bool,
    },
    /// 携带凭据但无效、吊销或无法解析。
    Invalid,
}

impl Subject {
    /// 返回已认证主体的用户标识。
    pub fn user_id(self) -> Option<i64> {
        match self {
            Subject::Authenticated { user_id, .. } => Some(user_id),
            Subject::Anonymous | Subject::Invalid => None,
        }
    }

    /// 判断是否为启用的全局管理员。
    pub fn is_global_admin(self) -> bool {
        matches!(
            self,
            Subject::Authenticated {
                role: Role::Admin,
                enabled: true,
                ..
            }
        )
    }
}

/// 授权入口面。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessSurface {
    /// 管理 API；有权者仍可查看停用状态。
    Management,
    /// 格式协议；停用仓库统一隐藏。
    Protocol,
}

/// 纯授权结果，由入口层稳定映射为 HTTP 语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationDecision {
    /// 允许执行动作。
    Allowed,
    /// 身份无效或已禁用，应返回 401。
    Unauthorized,
    /// 资源不存在或不得泄露存在性，应返回统一 404。
    Hidden,
    /// 已可读取资源但动作权限不足，应返回 403。
    Forbidden,
}

/// 仓库元数据领域实体，统计值由制品索引聚合查询提供。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    /// 仓库标识。
    pub id: i64,
    /// 全局唯一仓库名。
    pub name: String,
    /// 制品格式。
    pub format: Format,
    /// 仓库类型。
    pub repo_type: RepoType,
    /// 可见性。
    pub visibility: Visibility,
    /// 是否停用。
    pub disabled: bool,
    /// hosted 覆盖策略。
    pub overwrite_policy: Option<OverwritePolicy>,
    /// Maven hosted 版本策略。
    pub version_policy: Option<VersionPolicy>,
    /// proxy 上游地址。
    pub upstream_url: Option<String>,
    /// proxy 凭据运行时引用名。
    pub upstream_credential_ref: Option<String>,
    /// group 有序成员名。
    pub members: Vec<String>,
    /// 制品数量。
    pub object_count: i64,
    /// 制品总字节数。
    pub total_bytes: i64,
    /// 创建时间。
    pub created_at: String,
}

/// 协议入口使用的仓库上下文；group 成员已按配置顺序过滤为可读且启用的完整仓库。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolRepositoryContext {
    pub repository: Repository,
    pub members: Vec<Repository>,
}

/// ACL 条目领域实体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AclRecord {
    /// 用户标识。
    pub user_id: i64,
    /// 精确用户名。
    pub username: String,
    /// 授予权限。
    pub permission: Permission,
}

/// 无 IO 仓库授权判定。
pub fn authorize(
    subject: Subject,
    visibility: Visibility,
    granted: Option<Permission>,
    action: Permission,
    disabled: bool,
    surface: AccessSurface,
) -> AuthorizationDecision {
    if subject_is_unauthorized(subject) {
        return AuthorizationDecision::Unauthorized;
    }
    if disabled && surface == AccessSurface::Protocol {
        return AuthorizationDecision::Hidden;
    }
    if subject.is_global_admin() {
        return AuthorizationDecision::Allowed;
    }

    let effective = effective_permission(subject, visibility, granted);
    if visibility == Visibility::Private && effective.is_none() {
        return AuthorizationDecision::Hidden;
    }
    if effective.is_some_and(|permission| permission.includes(action)) {
        AuthorizationDecision::Allowed
    } else {
        AuthorizationDecision::Forbidden
    }
}

fn subject_is_unauthorized(subject: Subject) -> bool {
    matches!(
        subject,
        Subject::Invalid | Subject::Authenticated { enabled: false, .. }
    )
}

fn effective_permission(
    subject: Subject,
    visibility: Visibility,
    granted: Option<Permission>,
) -> Option<Permission> {
    let public = (visibility == Visibility::Public).then_some(Permission::Read);
    let acl = match subject {
        Subject::Authenticated {
            role: Role::User,
            enabled: true,
            ..
        } => granted,
        _ => None,
    };
    Permission::max(public, acl)
}

impl Permission {
    fn rank(self) -> u8 {
        match self {
            Permission::Read => 1,
            Permission::Write => 2,
            Permission::Admin => 3,
        }
    }

    fn includes(self, action: Permission) -> bool {
        self.rank() >= action.rank()
    }

    fn max(left: Option<Self>, right: Option<Self>) -> Option<Self> {
        match (left, right) {
            (Some(left), Some(right)) if right.rank() > left.rank() => Some(right),
            (Some(left), _) => Some(left),
            (None, right) => right,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AccessSurface, AuthorizationDecision, Permission, Subject, Visibility, authorize};
    use crate::domain::Role;

    fn permission_rank(permission: Permission) -> u8 {
        match permission {
            Permission::Read => 1,
            Permission::Write => 2,
            Permission::Admin => 3,
        }
    }

    fn expected(
        subject: Subject,
        visibility: Visibility,
        granted: Option<Permission>,
        action: Permission,
        disabled: bool,
        surface: AccessSurface,
    ) -> AuthorizationDecision {
        match subject {
            Subject::Invalid => return AuthorizationDecision::Unauthorized,
            Subject::Authenticated { enabled: false, .. } => {
                return AuthorizationDecision::Unauthorized;
            }
            _ => {}
        }
        if disabled && surface == AccessSurface::Protocol {
            return AuthorizationDecision::Hidden;
        }
        if matches!(
            subject,
            Subject::Authenticated {
                role: Role::Admin,
                ..
            }
        ) {
            return AuthorizationDecision::Allowed;
        }

        let public_read = visibility == Visibility::Public;
        let granted_rank = match subject {
            Subject::Authenticated {
                role: Role::User,
                enabled: true,
                ..
            } => granted.map(permission_rank).unwrap_or_default(),
            _ => 0,
        };
        let effective_rank = granted_rank.max(u8::from(public_read));
        if visibility == Visibility::Private && effective_rank == 0 {
            return AuthorizationDecision::Hidden;
        }
        if effective_rank >= permission_rank(action) {
            AuthorizationDecision::Allowed
        } else {
            AuthorizationDecision::Forbidden
        }
    }

    #[test]
    fn 授权矩阵穷举主体可见性动作状态与入口面() {
        let subjects = [
            Subject::Anonymous,
            Subject::Authenticated {
                user_id: 1,
                role: Role::User,
                enabled: true,
            },
            Subject::Authenticated {
                user_id: 2,
                role: Role::User,
                enabled: false,
            },
            Subject::Authenticated {
                user_id: 3,
                role: Role::Admin,
                enabled: true,
            },
            Subject::Authenticated {
                user_id: 4,
                role: Role::Admin,
                enabled: false,
            },
            Subject::Invalid,
        ];
        let visibilities = [Visibility::Public, Visibility::Private];
        let grants = [
            None,
            Some(Permission::Read),
            Some(Permission::Write),
            Some(Permission::Admin),
        ];
        let actions = [Permission::Read, Permission::Write, Permission::Admin];
        let surfaces = [AccessSurface::Management, AccessSurface::Protocol];

        for subject in subjects {
            for visibility in visibilities {
                for granted in grants {
                    for action in actions {
                        for disabled in [false, true] {
                            for surface in surfaces {
                                let actual = authorize(
                                    subject, visibility, granted, action, disabled, surface,
                                );
                                let expected = expected(
                                    subject, visibility, granted, action, disabled, surface,
                                );
                                assert_eq!(
                                    actual, expected,
                                    "主体={subject:?} 可见性={visibility:?} ACL={granted:?} 动作={action:?} 停用={disabled} 入口={surface:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn 私有仓库无读取权隐藏_已有读取权但动作不足则禁止() {
        let user = Subject::Authenticated {
            user_id: 1,
            role: Role::User,
            enabled: true,
        };
        assert_eq!(
            authorize(
                user,
                Visibility::Private,
                None,
                Permission::Admin,
                false,
                AccessSurface::Management,
            ),
            AuthorizationDecision::Hidden
        );
        assert_eq!(
            authorize(
                user,
                Visibility::Private,
                Some(Permission::Read),
                Permission::Admin,
                false,
                AccessSurface::Management,
            ),
            AuthorizationDecision::Forbidden
        );
    }

    #[test]
    fn 停用仓库协议面隐藏而管理面对有权者可见() {
        let admin = Subject::Authenticated {
            user_id: 1,
            role: Role::Admin,
            enabled: true,
        };
        assert_eq!(
            authorize(
                admin,
                Visibility::Private,
                None,
                Permission::Admin,
                true,
                AccessSurface::Protocol,
            ),
            AuthorizationDecision::Hidden
        );
        assert_eq!(
            authorize(
                admin,
                Visibility::Private,
                None,
                Permission::Admin,
                true,
                AccessSurface::Management,
            ),
            AuthorizationDecision::Allowed
        );
    }
}
