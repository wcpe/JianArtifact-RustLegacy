//! 控制台设置可编辑端点（FR-87 只读 + FR-88 可编辑热替换）：仅 Admin 读取 / 修改脱敏后的网络代理
//! （FR-84）+ 在线更新（FR-85）配置，PATCH 即时生效、无须重启。
//!
//! 设计要点：
//! - **薄 handler**：只做鉴权编排（仅 Admin）、读热替换槽组装脱敏 DTO / 校验后换槽、返回 JSON；无业务逻辑。
//! - **可编辑 + 热替换（FR-88，ADR-0022）**：`GET` 回显热替换槽**当前生效值**（含运行时 PATCH 在内）；
//!   `PATCH` 校验后锁外重建出站 client、原子换槽，下个出站请求即用新代理 / 新更新开关，无须重启。
//! - **校验失败不改状态**：校验未过返回 400 且**不替换**现有生效值（GET 仍返回旧值）。
//! - **脱敏红线**：响应**绝不含任何凭据**——代理 URL 经 [`sanitize_proxy_url`] 去 `user:pass@`；
//!   更新 token 只回 `has_token: bool`，绝不回显 token 本体。
//! - **凭据持久化边界**：代理密码经 ADR-0030 专用通道加密落库，update token 只入热替换槽；二者均
//!   **不写回 TOML / 不进日志 / 不回显**。

use axum::{
    extract::{rejection::JsonRejection, State},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::config::{EditableUpdate, NetworkProxyConfig};

use super::{ApiError, AppState, Identity};

/// 去除 URL 中的 userinfo（`scheme://user:pass@host` → `scheme://host`）。
///
/// 仅做凭据脱敏，不重排其余部分：
/// - userinfo 仅存在于 authority 段（`scheme://userinfo@host` 中、host 路径分隔 `/` 之前）。
///   取该段内最后一个 `@` 为 userinfo 与 host 的分界，去除其前段；保留 scheme、host、port、
///   path、query 原样。
/// - authority 段内无 `@`（含 `@` 仅出现在 path/query 时）：原样返回，不误删。
/// - 空串 / 异常形态：原样返回，不 panic（脱敏不应引入新错误）。
pub fn sanitize_proxy_url(url: &str) -> String {
    // authority 段起点：scheme 后 `//` 之后；无 `//`（非标准 URL）时整串视作 authority 起点。
    let authority_start = match url.find("://") {
        Some(scheme_end) => scheme_end + 3,
        None => 0,
    };
    // authority 段终点：authority 起点之后首个 `/`（path 起点）；无 path 时到串尾。
    let authority_end = url[authority_start..]
        .find('/')
        .map(|rel| authority_start + rel)
        .unwrap_or(url.len());
    // 仅在 authority 段内找 userinfo 分界 `@`（取最后一个，兼容口令含 `@`）；无则无 userinfo
    let Some(rel_at) = url[authority_start..authority_end].rfind('@') else {
        return url.to_string();
    };
    let at_pos = authority_start + rel_at;
    // 拼接：authority 起点之前（含 `scheme://`）+ `@` 之后（host 起点）
    let mut sanitized = String::with_capacity(url.len());
    sanitized.push_str(&url[..authority_start]);
    sanitized.push_str(&url[at_pos + 1..]);
    sanitized
}

/// RFC 3986 unreserved 字符：`ALPHA / DIGIT / "-" / "." / "_" / "~"`，编码时保留不转义。
fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

/// 把 userinfo 组件（用户名 / 密码）按 RFC 3986 百分号编码：非 unreserved 字符一律转义。
///
/// 比 RFC 的 userinfo 允许集更严（连 sub-delims 也编码），确保 `:` `@` `/` 等保留字符被转义，
/// 重组的 `scheme://user:pass@host` 不产生歧义（口令含特殊字符也安全）。
fn percent_encode_userinfo(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if is_unreserved(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

/// 百分号解码（GET 回显用户名时把存储的编码还原）。非法转义序列原样保留，不 panic。
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 在脱敏后的 `scheme://host...` 中插入 `userinfo@`（用户名已百分号编码）。
///
/// 取 authority 起点（`scheme://` 之后；无 scheme 时串首），在该处插入 `userinfo@`。
fn insert_userinfo(host_url: &str, userinfo: &str) -> String {
    let authority_start = match host_url.find("://") {
        Some(scheme_end) => scheme_end + 3,
        None => 0,
    };
    let mut out = String::with_capacity(host_url.len() + userinfo.len() + 1);
    out.push_str(&host_url[..authority_start]);
    out.push_str(userinfo);
    out.push('@');
    out.push_str(&host_url[authority_start..]);
    out
}

/// 据单代理三字段 + 当前存储值，重建含凭据的存储 URL（FR-100，纯函数、可穷举测试）。
///
/// 返回 `None` 表示清除该代理。规则（ADR-0024 / spec §3.3）：
/// 1. `patch.url` 空白 / 缺省 → `None`（清除；用户名 / 密码忽略）。
/// 2. 否则 `host = sanitize_proxy_url(url)`（去掉用户误带的 userinfo）。
/// 3. `username = patch.username.unwrap_or_default().trim()`。
/// 4. 密码三态：缺省=保留现有（仅当 `current` 用户名与本次 `username` 一致时沿用其密码，
///    否则视为无密码）/ 空串=无密码 / 非空=设为该值。
/// 5. 组装：`username` 空 → 直接 host（无 userinfo，即便给了密码也忽略）；
///    否则在 `scheme://` 后插入 `username[:password]@`，userinfo 按 RFC 3986 百分号编码。
fn rebuild_proxy_url(patch: &ProxyEntryPatch, current: Option<&str>) -> Option<String> {
    // 规则 1：URL 空白 / 缺省即清除
    let url = patch.url.as_deref().map(str::trim).unwrap_or("");
    if url.is_empty() {
        return None;
    }
    // 规则 2：去掉用户误带的 userinfo，只留 scheme://host:port[/path]
    let host = sanitize_proxy_url(url);
    // 规则 3：用户名 trim
    let username = patch.username.as_deref().unwrap_or_default().trim();

    // 规则 4：密码三态
    let password: Option<String> = match patch.password.as_deref() {
        // 缺省：保留现有——仅当当前存储值的用户名与本次 username 一致时沿用其密码
        None => {
            let (cur_user, _) = current
                .map(parse_proxy_credentials)
                .unwrap_or((None, false));
            if !username.is_empty() && cur_user.as_deref() == Some(username) {
                current.and_then(parse_proxy_password)
            } else {
                None
            }
        }
        // 空串：清空密码
        Some("") => None,
        // 非空：设置为新密码
        Some(p) => Some(p.to_string()),
    };

    // 规则 5：组装 userinfo（用户名为空则无 userinfo，忽略密码）
    if username.is_empty() {
        return Some(host);
    }
    let mut userinfo = percent_encode_userinfo(username);
    if let Some(p) = password {
        userinfo.push(':');
        userinfo.push_str(&percent_encode_userinfo(&p));
    }
    Some(insert_userinfo(&host, &userinfo))
}

/// 从存储 URL 解析（已解码的）密码本体（仅 [`rebuild_proxy_url`] 内部「保留现有密码」用）。
///
/// 与 [`parse_proxy_credentials`] 同口径取 userinfo，但返回密码明文（不外泄、仅用于重组存储 URL）。
fn parse_proxy_password(url: &str) -> Option<String> {
    let authority_start = match url.find("://") {
        Some(scheme_end) => scheme_end + 3,
        None => 0,
    };
    let authority_end = url[authority_start..]
        .find('/')
        .map(|rel| authority_start + rel)
        .unwrap_or(url.len());
    let rel_at = url[authority_start..authority_end].rfind('@')?;
    let userinfo = &url[authority_start..authority_start + rel_at];
    let (_user, pass_enc) = userinfo.split_once(':')?;
    Some(percent_decode(pass_enc))
}

// ===== 代理凭据加密落库持久化（ADR-0030）：拆分落库 + 启动恢复 =====

/// `app_settings` 中网络代理的持久化键（专用加密通道，不入 config_overlay 的 DYNAMIC_KEYS 白名单）。
pub const PROXY_SETTING_KEY: &str = "network.proxy";

/// 单代理的落库形态（ADR-0030）：URL 脱敏 host（非密钥，明文）+ 用户名（标识，明文）+ 密码密文。
///
/// **密码只以密文落库**（`password_enc`，XChaCha20-Poly1305）；URL 已脱敏不含凭据、用户名是连接标识，
/// 二者属非密钥明文落库（沿用 ADR-0024）。三字段可空：未配置该代理则整项为 `None`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedProxyEntry {
    /// 脱敏后的代理 URL（`scheme://host:port`，无 userinfo）；可空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// 用户名（连接标识、非密钥，明文）；无用户名为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// 密码密文（`base64(nonce ‖ ciphertext)`）；**绝不是明文**，无密码为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_enc: Option<String>,
}

/// 网络代理整体落库形态（ADR-0030；FR-146 / ADR-0042 收敛单代理）。
///
/// 新写入只落 `proxy` + `no_proxy`；旧 `http` / `https` / `all` 三项仅为历史落库 JSON 的
/// 读取兼容保留（恢复时按 `proxy > all > https > http` 折叠，见 [`from_persisted`]）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PersistedProxy {
    /// 通用代理拆分形态（FR-146 单代理）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<PersistedProxyEntry>,
    /// 已废弃：旧 HTTP 代理拆分形态（仅读取兼容）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http: Option<PersistedProxyEntry>,
    /// 已废弃：旧 HTTPS 代理拆分形态（仅读取兼容）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub https: Option<PersistedProxyEntry>,
    /// 已废弃：旧全 scheme 兜底代理拆分形态（仅读取兼容）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all: Option<PersistedProxyEntry>,
    /// 直连绕过列表（无凭据，明文）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_proxy: Option<String>,
}

impl PersistedProxy {
    /// 判断是否「空代理」（各项全空）——空则启动恢复时不覆盖文件默认、PATCH 时删键。
    fn is_empty(&self) -> bool {
        self.proxy.is_none()
            && self.http.is_none()
            && self.https.is_none()
            && self.all.is_none()
            && self.no_proxy.is_none()
    }
}

/// 从存储 URL 解析（已解码的）密码明文：与 [`parse_proxy_password`] 同口径，供拆分落库用。
///
/// 把单条存储 URL（含 `user:pass@`）拆为落库形态：URL 脱敏去凭据、用户名明文、密码用 `key`
/// 加密为密文。无 URL 返回 `None`（清除该代理）。
fn split_proxy_entry(stored: Option<&str>, key: &[u8; 32]) -> Option<PersistedProxyEntry> {
    let url = stored?;
    let (username, _has_pw) = parse_proxy_credentials(url);
    let password_enc =
        parse_proxy_password(url).map(|pw| crate::crypto_box::encrypt_secret(key, &pw));
    Some(PersistedProxyEntry {
        url: Some(sanitize_proxy_url(url)),
        username,
        password_enc,
    })
}

/// 把当前生效的内存态代理配置拆分为加密落库形态（ADR-0030；FR-146 单代理）。
///
/// 纯函数：URL 脱敏、用户名明文、密码加密；`no_proxy` 原样。密码绝不以明文出现在产物里。
/// 只写新形态 `proxy` 项，旧三项一律不再写出。
pub fn to_persisted(proxy: &NetworkProxyConfig, key: &[u8; 32]) -> PersistedProxy {
    PersistedProxy {
        proxy: split_proxy_entry(proxy.effective_url(), key),
        http: None,
        https: None,
        all: None,
        no_proxy: proxy.no_proxy.clone(),
    }
}

/// 把单条落库形态解密重建为含凭据的存储 URL（启动恢复用）。
///
/// 复用 [`rebuild_proxy_url`] 的组装规则：用 URL（脱敏 host）+ 用户名 + 解密后的密码三字段重组。
/// 解密失败（密钥轮换 / 密文损坏）降级为「无密码」（密码三态置空串），不阻断、不 panic。
/// 返回 `(重建 URL, 是否发生解密失败需 WARN)`。
fn rebuild_persisted_entry(entry: &PersistedProxyEntry, key: &[u8; 32]) -> (Option<String>, bool) {
    // 密码三态：无密文=空串（无密码）；有密文则解密，失败降级为空串并标记 WARN
    let (password, degraded) = match entry.password_enc.as_deref() {
        None => (Some(String::new()), false),
        Some(enc) => match crate::crypto_box::decrypt_secret(key, enc) {
            Some(pw) => (Some(pw), false),
            None => (Some(String::new()), true),
        },
    };
    let patch = ProxyEntryPatch {
        url: entry.url.clone(),
        username: entry.username.clone(),
        password,
    };
    // current 传 None：落库形态自含全部三字段，无须沿用「现有密码」
    (rebuild_proxy_url(&patch, None), degraded)
}

/// 把落库形态解密重建为内存态 [`NetworkProxyConfig`]（启动恢复，ADR-0030；FR-146 单代理）。
///
/// 旧三项落库 JSON 按优先级 `proxy > all > https > http` 折叠为单代理（与 ADR-0042 配置折叠
/// 同序），凭据不丢。解密失败即降级为无密码并记一条 WARN（不泄露密文 / 密钥），不阻断启动。
pub fn from_persisted(persisted: &PersistedProxy, key: &[u8; 32]) -> NetworkProxyConfig {
    // 折叠优先级与配置旧键一致：新形态 proxy 为真源，旧三项按 all > https > http 兜底
    let entry = persisted
        .proxy
        .as_ref()
        .or(persisted.all.as_ref())
        .or(persisted.https.as_ref())
        .or(persisted.http.as_ref());
    let (url, degraded) = match entry {
        Some(e) => rebuild_persisted_entry(e, key),
        None => (None, false),
    };
    if degraded {
        tracing::warn!(
            "已落库的网络代理密码解密失败（.jwt_secret 可能已轮换），代理降级为无密码，请在设置页重填"
        );
    }
    NetworkProxyConfig {
        url,
        http: None,
        https: None,
        all: None,
        no_proxy: persisted.no_proxy.clone(),
    }
}

/// 从 `app_settings` 读出的网络代理 JSON 解密重建内存态代理配置（启动恢复入口，ADR-0030）。
///
/// `value_json` 为 [`PROXY_SETTING_KEY`] 对应的存储 JSON；解析失败只 WARN 回落 `None`（不阻断启动）。
/// 返回 `Some(cfg)` 表示有落库代理需覆盖文件默认；`None` 表示无 / 损坏 / 空代理，保持文件默认。
pub fn restore_proxy_from_db(value_json: &str, key: &[u8; 32]) -> Option<NetworkProxyConfig> {
    let persisted = match serde_json::from_str::<PersistedProxy>(value_json) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(原因 = %e, "解析已落库网络代理配置失败，回落文件默认");
            return None;
        }
    };
    if persisted.is_empty() {
        return None;
    }
    Some(from_persisted(&persisted, key))
}

/// 单代理视图（脱敏后，FR-100）：URL 去凭据 + 用户名回显 + 是否已配置密码。
///
/// **密码绝不回显**：仅以 `has_password` 暴露是否已配置（ADR-0024「用户名回显、密码不回显」）。
#[derive(Debug, Serialize)]
pub struct ProxyEntryView {
    /// 脱敏后的代理 URL（`scheme://host:port`，无 userinfo）；未配置为 `None`。
    pub url: Option<String>,
    /// 回显用户名（连接标识、非密钥；无用户名或未配置为 `None`）。
    pub username: Option<String>,
    /// 是否已配置密码：**仅布尔，绝不回显密码本体**。
    pub has_password: bool,
}

/// 网络代理视图（脱敏后，FR-146 单代理）：URL + 用户名 + has_password 三字段。
#[derive(Debug, Serialize)]
pub struct NetworkProxyView {
    /// 通用出站代理（接受 `http://` / `https://` / `socks5://` / `socks5h://`）。
    pub proxy: ProxyEntryView,
    /// 直连绕过列表（无凭据，原样）。
    pub no_proxy: Option<String>,
}

/// 从存储代理 URL 解析回显用户名与是否含密码（GET 用，FR-100）。
///
/// 与 [`sanitize_proxy_url`] 同口径：只看 authority 段（`scheme://...host` 前、首个 `/` 之前）的
/// userinfo（authority 内最后一个 `@` 之前的部分），取其中首个 `:` 分隔的用户名与密码。
/// userinfo 中的用户名 / 密码经 RFC 3986 百分号编码存储，这里按同口径解码回显。
/// 返回 `(回显用户名, 是否含密码)`：无 userinfo / 用户名为空 → `(None, false)`。
fn parse_proxy_credentials(url: &str) -> (Option<String>, bool) {
    let authority_start = match url.find("://") {
        Some(scheme_end) => scheme_end + 3,
        None => 0,
    };
    let authority_end = url[authority_start..]
        .find('/')
        .map(|rel| authority_start + rel)
        .unwrap_or(url.len());
    // userinfo 与 host 的分界取最后一个 `@`（兼容口令含 `@`，与脱敏同口径）
    let Some(rel_at) = url[authority_start..authority_end].rfind('@') else {
        return (None, false);
    };
    let userinfo = &url[authority_start..authority_start + rel_at];
    // 用户名 / 密码以首个 `:` 分隔（用户名内 `:` 已被百分号编码，不会误分）
    let (user_enc, has_password) = match userinfo.split_once(':') {
        Some((u, _pass)) => (u, true),
        None => (userinfo, false),
    };
    let username = percent_decode(user_enc);
    if username.is_empty() {
        // 无用户名（即便有 `:password`）视为无凭据回显——存储侧也不会单挂密码
        (None, false)
    } else {
        (Some(username), has_password)
    }
}

/// 把存储 URL 解析为单代理视图（脱敏 + 用户名回显 + has_password）。
fn proxy_entry_view(stored: Option<&str>) -> ProxyEntryView {
    match stored {
        None => ProxyEntryView {
            url: None,
            username: None,
            has_password: false,
        },
        Some(url) => {
            let (username, has_password) = parse_proxy_credentials(url);
            ProxyEntryView {
                url: Some(sanitize_proxy_url(url)),
                username,
                has_password,
            }
        }
    }
}

/// 在线更新视图（脱敏后）。
#[derive(Debug, Serialize)]
pub struct UpdateView {
    /// 是否启用在线更新（出站开关）。
    pub enabled: bool,
    /// 仓库源（`owner/repo`）。
    pub repo: String,
    /// GitHub API 基址。
    pub api_base_url: String,
    /// 重启模式（`self` / `exit`）。
    pub restart_mode: String,
    /// 更新通道（`stable` / `prerelease`，FR-89）。
    pub channel: String,
    /// 是否已配置访问 token：**仅布尔，绝不回显 token 本体**。
    pub has_token: bool,
    /// 是否有可回滚的上一版本备份（FR-104）：持久回滚备份存在时为 `true`，供控制台启用回滚按钮。
    pub rollback_available: bool,
}

/// 设置页聚合视图（脱敏后）。
#[derive(Debug, Serialize)]
pub struct SettingsView {
    /// 当前运行版本（编译期注入）。
    pub current_version: String,
    /// 网络代理配置（脱敏）。
    pub network_proxy: NetworkProxyView,
    /// 在线更新配置（脱敏）。
    pub update: UpdateView,
}

/// 据热替换槽当前值组装脱敏视图（GET 与 PATCH 成功后复用）。
fn current_view(state: &AppState) -> SettingsView {
    let snapshot = state.settings.network.snapshot();
    let proxy = &snapshot.proxy;
    let update = state.settings.update();

    SettingsView {
        current_version: crate::version::build_version().to_string(),
        network_proxy: NetworkProxyView {
            proxy: proxy_entry_view(proxy.effective_url()),
            no_proxy: proxy.no_proxy.clone(),
        },
        update: UpdateView {
            enabled: update.enabled,
            repo: update.repo.clone(),
            api_base_url: update.api_base_url.clone(),
            restart_mode: update.restart_mode.clone(),
            channel: update.channel.clone(),
            // 仅暴露是否已配置 token，绝不回显 token 本体
            has_token: update.token.is_some(),
            // 持久回滚备份是否存在（FR-104）：定位 current_exe 失败时降级为 false（不暴露路径）
            rollback_available: std::env::current_exe()
                .map(|exe| crate::update::rollback_available(&exe))
                .unwrap_or(false),
        },
    }
}

/// 读取脱敏后的网络代理 + 在线更新配置与当前版本（仅 Admin）。
///
/// 未认证 401、非管理员 403（复用 [`Identity::require_admin`]）。读**热替换槽当前生效值**
/// （含运行时 PATCH 在内），代理 URL 去凭据、token 只回 `has_token`，响应绝不含任何凭据。
pub async fn get_settings(
    State(state): State<AppState>,
    identity: Identity,
) -> Result<Json<SettingsView>, ApiError> {
    identity.require_admin()?;
    Ok(Json(current_view(&state)))
}

/// 单代理编辑项（FR-100）：URL（脱敏 host，无凭据）+ 用户名 + 密码（三态）。
///
/// 密码沿用 update token 的三态语义：缺省=保留现有 / 空串=清空 / 非空=设置。
/// 用户名是连接标识，可随 URL 一起回显与编辑（密码绝不回显）。
#[derive(Debug, Default, Deserialize)]
pub struct ProxyEntryPatch {
    /// 代理 URL（host，无凭据）；`null` / 缺省 / 空白视为清除该代理（用户名 / 密码一并忽略）。
    #[serde(default)]
    pub url: Option<String>,
    /// 用户名（连接标识）；缺省 / 空视为无用户（无用户即便给了密码也忽略）。
    #[serde(default)]
    pub username: Option<String>,
    /// 密码三态：缺省 / `null`=保留现有（同用户名时沿用）/ 空串=清空 / 非空=设置。
    #[serde(default)]
    pub password: Option<String>,
}

/// 网络代理编辑请求（FR-146 单代理）：三字段 + 直连绕过列表。
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkProxyPatch {
    /// 通用出站代理（接受 `http://` / `https://` / `socks5://` / `socks5h://`）。
    #[serde(default)]
    pub proxy: ProxyEntryPatch,
    /// 直连绕过列表。
    #[serde(default)]
    pub no_proxy: Option<String>,
}

/// 在线更新编辑请求。
#[derive(Debug, Deserialize)]
pub struct UpdatePatch {
    /// 是否启用在线更新（出站开关）。
    pub enabled: bool,
    /// 仓库源（`owner/repo`）。
    pub repo: String,
    /// GitHub API 基址。
    pub api_base_url: String,
    /// 重启模式（`self` / `exit`）。
    pub restart_mode: String,
    /// 更新通道（`stable` / `prerelease`，FR-89）。
    pub channel: String,
    /// 访问 token 编辑语义（GET 不回显 token，故区分三态）：
    /// - 缺省 / `null`：**保留**当前 token 不变；
    /// - 空串 `""`：**清空** token；
    /// - 非空串：**设置**为新 token（只入内存槽、不入库 / 不进日志 / 不回显）。
    #[serde(default)]
    pub token: Option<String>,
}

/// 设置编辑请求体：网络代理与在线更新两块均**可选**，按提供的块部分更新（FR-109 拆分后，
/// 设置页只发 `network_proxy`、系统页只发 `update`；两块都给仍整体替换，向后兼容）。
#[derive(Debug, Default, Deserialize)]
pub struct SettingsPatch {
    /// 网络代理编辑项（缺省则不动代理）。
    #[serde(default)]
    pub network_proxy: Option<NetworkProxyPatch>,
    /// 在线更新编辑项（缺省则不动在线更新）。
    #[serde(default)]
    pub update: Option<UpdatePatch>,
}

/// 把空白字符串归一为 `None`（前端清空输入即不配置该代理项）。
fn normalize_blank(v: Option<String>) -> Option<String> {
    v.and_then(|s| if s.trim().is_empty() { None } else { Some(s) })
}

/// 把当前生效代理拆分加密后落库 `app_settings`（ADR-0030）：空代理删键、否则 upsert 密文形态。
///
/// 用 `state.jwt` 派生的子密钥加密密码；密钥与明文密码绝不入库 / 进日志。落库 / 序列化失败只 WARN，
/// 不阻断热替换（即时生效优先）。
async fn persist_proxy(state: &AppState, proxy: &NetworkProxyConfig) {
    let key = state.jwt.derive_key(crate::crypto_box::PROXY_KEY_DOMAIN);
    let persisted = to_persisted(proxy, &key);
    if persisted.is_empty() {
        // 空代理：删键回落文件默认（删不存在键不报错）
        if let Err(e) = state.meta.delete_setting(PROXY_SETTING_KEY).await {
            tracing::warn!(原因 = %e, "清空网络代理落库失败，热替换仍生效（重启回落文件）");
        }
        return;
    }
    match serde_json::to_string(&persisted) {
        Ok(json) => {
            if let Err(e) = state.meta.upsert_setting(PROXY_SETTING_KEY, &json).await {
                tracing::warn!(原因 = %e, "网络代理落库失败，热替换仍生效（重启回落文件）");
            }
        }
        Err(e) => {
            tracing::warn!(原因 = %e, "网络代理配置序列化失败，跳过落库，热替换仍生效");
        }
    }
}

/// 编辑网络代理 + 在线更新配置（仅 Admin），校验通过即时生效、无须重启。
///
/// 校验失败返回 400 且**不改变**现有生效值（GET 仍返回旧值）；成功后锁外重建出站 client、原子换槽，
/// 下个出站请求即用新代理 / 新更新开关。代理密码加密落库，update token 只入内存；均不写 TOML / 不回显。
/// 未认证 401、非管理员 403。
pub async fn patch_settings(
    State(state): State<AppState>,
    identity: Identity,
    body: Result<Json<SettingsPatch>, JsonRejection>,
) -> Result<Json<SettingsView>, ApiError> {
    identity.require_admin()?;
    let Json(patch) = body.map_err(|_| ApiError::BadRequest("请求体 JSON 格式非法".to_string()))?;
    // 同一进程内串行完成「读取当前值 → 换槽 → DB 持久化」，避免并发 PATCH 使内存与 DB 最终顺序倒置。
    let _patch_guard = state.settings.lock_patch().await;

    // 预校验在线更新（若提供）：token 三态——缺省保留、空串清空、非空设置；非法即拒、不触碰任何现有生效值。
    let new_update = match &patch.update {
        Some(up) => {
            let current_update = state.settings.update();
            let new_token = match up.token.as_deref() {
                None => current_update.token.clone(),
                Some(t) if t.trim().is_empty() => None,
                Some(t) => Some(t.to_string()),
            };
            let nu = EditableUpdate {
                enabled: up.enabled,
                repo: up.repo.trim().to_string(),
                api_base_url: up.api_base_url.trim().to_string(),
                restart_mode: up.restart_mode.trim().to_string(),
                channel: up.channel.trim().to_string(),
                // 下载超时不在面板可调，沿用当前值（与 ADR-0021 启动期口径一致）
                download_timeout_secs: current_update.download_timeout_secs,
                token: new_token,
            };
            nu.validate()
                .map_err(|reason| ApiError::BadRequest(format!("在线更新配置非法：{reason}")))?;
            Some(nu)
        }
        None => None,
    };

    // 应用网络代理（若提供）：经 rebuild_proxy_url 据三字段 + 当前存储值重建含凭据的存储 URL；
    // no_proxy 空白归一为不配置。锁外重建 client，失败即拒（现有 client 仍生效）。凭据只入内存槽、
    // 不回显、不写回 TOML / 不进日志（加密落库见 ADR-0030）。
    if let Some(np) = &patch.network_proxy {
        let current_proxy = state.settings.network.snapshot().proxy.clone();
        let new_proxy = NetworkProxyConfig {
            url: rebuild_proxy_url(&np.proxy, current_proxy.effective_url()),
            http: None,
            https: None,
            all: None,
            no_proxy: normalize_blank(np.no_proxy.clone()),
        };
        // replace_proxy 取走所有权，故先 clone 一份留作落库（代理配置小、clone 廉价）。
        let persist_proxy_cfg = new_proxy.clone();
        state
            .settings
            .network
            .replace_proxy(new_proxy)
            .map_err(|reason| ApiError::BadRequest(format!("网络代理配置非法：{reason}")))?;
        // 持久化网络代理到 app_settings（ADR-0030）：URL 脱敏 host + 用户名明文 + 密码密文；
        // 密钥经 JwtSigner::derive_key 域分隔派生（真源 .jwt_secret，绝不入库）。落库失败只 WARN、
        // 不阻断热替换。空代理则删键，恢复为文件默认。
        persist_proxy(&state, &persist_proxy_cfg).await;
    }

    // 应用在线更新（若提供，已校验）：换槽即时生效；非密钥字段落库（FR-106，ADR-0028），token 自动剔除
    // （真源是 env、绝不入库）。落库失败只 WARN，不阻断热替换（重启回落上次入库 / 文件 / env）。
    if let Some(nu) = new_update {
        state.settings.replace_update(nu.clone());
        let tunables = crate::config_overlay::UpdateTunables::from_editable(&nu);
        match serde_json::to_string(&tunables) {
            Ok(json) => {
                if let Err(e) = state.meta.upsert_setting("update", &json).await {
                    tracing::warn!(原因 = %e, "在线更新非密钥配置落库失败，热替换仍生效（重启回落 env / 文件）");
                }
            }
            Err(e) => {
                tracing::warn!(原因 = %e, "在线更新配置序列化失败，跳过落库，热替换仍生效");
            }
        }
    }

    // 记一条管理动作日志（仅记动作，不含任何凭据明文）
    tracing::info!(操作者 = %identity.actor_name(), "管理员更新了设置，已即时生效");

    Ok(Json(current_view(&state)))
}

/// 出站代理连通性测试请求体（FR-128；FR-147 增可选表单代理）。
#[derive(Debug, Deserialize)]
pub struct ProxyTestRequest {
    /// 目标测试 URL（仅接受 http/https scheme）。
    pub url: String,
    /// 可选：表单当前填写的代理（FR-147）。给出即用其临时构造 client 测试（无需先保存）；
    /// 密码缺省时回落已存凭据（同用户名才沿用，复用 PATCH 三态）；`url` 空白 = 测直连。
    /// 缺省整个字段则测当前已生效代理（兼容旧调用）。
    #[serde(default)]
    pub proxy: Option<ProxyEntryPatch>,
}

/// 出站代理连通性测试响应（FR-128）。
#[derive(Debug, Serialize)]
pub struct ProxyTestResult {
    /// 是否连通：能收到响应即为 true，连接失败 / 超时为 false。
    pub ok: bool,
    /// HTTP 响应状态码（仅 ok=true 时有值）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// 往返耗时（毫秒）。
    pub elapsed_ms: u64,
    /// 失败原因（仅 ok=false 时有值，不含凭据 / 代理 URL 明文）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 出站代理连通性测试（FR-128，仅 Admin；FR-147 支持测表单当前填写的代理）。
///
/// 请求体给出 `proxy` 时据其临时构造 client（密码缺省回落已存凭据），否则取当前生效出站
/// client（含代理配置），对用户给定 URL 发 GET，返回连通性。
/// 仅接受 http/https scheme（防 SSRF file/ftp 等）；仅访问用户给定 URL，不外发任何使用数据（ADR-0009）。
pub async fn proxy_test(
    State(state): State<AppState>,
    identity: Identity,
    Json(req): Json<ProxyTestRequest>,
) -> Result<Json<ProxyTestResult>, ApiError> {
    identity.require_admin()?;

    let url = reqwest::Url::parse(req.url.trim())
        .map_err(|_| ApiError::BadRequest("URL 格式非法".to_string()))?;

    // 校验 URL：仅接受 http/https scheme 且必须含 host（防 SSRF file/ftp 等非出站协议）
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ApiError::BadRequest(
            "仅支持 http:// 或 https:// 开头的 URL".to_string(),
        ));
    }
    if url.host().is_none() {
        return Err(ApiError::BadRequest("URL 格式非法".to_string()));
    }

    // FR-147：给出表单代理即临时构造 client 测表单值（无需先保存）；否则取当前生效 client
    let client = match &req.proxy {
        Some(patch) if patch.url.as_deref().map(str::trim).unwrap_or("").is_empty() => {
            // 显式空代理表示测直连，需关闭 reqwest 自动系统代理探测
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .no_proxy()
                .build()
                .map_err(|_| ApiError::BadRequest("代理配置非法：无法构造直连客户端".to_string()))?
        }
        Some(patch) => {
            // 密码缺省回落已存凭据（同用户名才沿用），与 PATCH 三态同一纯函数
            let current = state.settings.network.snapshot().proxy.clone();
            let temp = crate::config::NetworkProxyConfig {
                url: rebuild_proxy_url(patch, current.effective_url()),
                http: None,
                https: None,
                all: None,
                no_proxy: None,
            };
            // 临时 client 仅本次测试用；构造失败即 400（错误信息不含凭据）
            crate::config::build_outbound_client(std::time::Duration::from_secs(10), &temp)
                .map_err(|reason| ApiError::BadRequest(format!("代理配置非法：{reason}")))?
        }
        // 兼容缺省 proxy 的旧调用：取当前生效出站 client，读锁极短、锁外发请求
        None => state.settings.network.client(),
    };

    let start = std::time::Instant::now();
    // 带 10s 超时的 GET 请求（覆盖 client 默认超时，专用测试超时）
    let result = client
        .get(url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await;
    let elapsed_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(resp) => {
            let status = resp.status().as_u16();
            tracing::info!(
                操作者 = %identity.actor_name(),
                状态码 = status,
                耗时毫秒 = elapsed_ms,
                "代理连通性测试成功"
            );
            Ok(Json(ProxyTestResult {
                ok: true,
                status: Some(status),
                elapsed_ms,
                error: None,
            }))
        }
        Err(e) => {
            // 错误描述不含代理 URL / 凭据（reqwest 错误消息不暴露代理配置）
            let error_msg = if e.is_timeout() {
                "连接超时".to_string()
            } else if e.is_connect() {
                "连接失败".to_string()
            } else if e.is_builder() {
                "URL 格式非法".to_string()
            } else {
                "请求失败".to_string()
            };
            tracing::info!(
                操作者 = %identity.actor_name(),
                耗时毫秒 = elapsed_ms,
                "代理连通性测试失败"
            );
            Ok(Json(ProxyTestResult {
                ok: false,
                status: None,
                elapsed_ms,
                error: Some(error_msg),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{测试用状态, 读_json};
    use super::*;
    use crate::auth::hash_password;
    use crate::meta::Role;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tower::ServiceExt;

    // ===== sanitize_proxy_url 纯函数穷举 =====

    #[test]
    fn 脱敏_去除带端口的_userinfo() {
        assert_eq!(
            sanitize_proxy_url("http://user:pass@proxy.internal:8080"),
            "http://proxy.internal:8080"
        );
    }

    #[test]
    fn 脱敏_去除仅用户名的_userinfo() {
        assert_eq!(
            sanitize_proxy_url("https://alice@proxy.internal"),
            "https://proxy.internal"
        );
    }

    #[test]
    fn 脱敏_无_userinfo_原样返回() {
        assert_eq!(
            sanitize_proxy_url("http://proxy.internal:8080"),
            "http://proxy.internal:8080"
        );
    }

    #[test]
    fn 脱敏_空串不_panic_原样返回() {
        assert_eq!(sanitize_proxy_url(""), "");
    }

    #[test]
    fn 脱敏_无_scheme_仍去除_userinfo() {
        // 无 scheme 的异常形态：把 `@` 前整段视作 userinfo 去除，结果不含凭据
        assert_eq!(sanitize_proxy_url("user:pass@host:8080"), "host:8080");
    }

    #[test]
    fn 脱敏_path_中的_at_不误删() {
        // `@` 出现在 path 段（authority 之后）：非 userinfo，原样返回
        assert_eq!(
            sanitize_proxy_url("http://proxy.internal/path@x"),
            "http://proxy.internal/path@x"
        );
    }

    #[test]
    fn 脱敏_多个_at_取最后一个_authority_分隔() {
        // 密码中含 `@`（少见但合法）：以最后一个 `@` 为 userinfo 与 host 分界
        assert_eq!(
            sanitize_proxy_url("http://user:p@ss@proxy.internal:8080"),
            "http://proxy.internal:8080"
        );
    }

    // ===== GET /api/v1/settings 端点鉴权 + 脱敏 =====

    /// 在状态库内建一个指定角色用户并签发其会话 JWT。
    async fn 签发令牌(state: &AppState, name: &str, role: Role) -> String {
        let uid = state
            .meta
            .create_user(name, &hash_password("pw").unwrap(), role)
            .await
            .unwrap();
        state.jwt.issue(&uid, name, role).unwrap()
    }

    /// 便捷：带可选 Bearer 令牌 GET 设置端点。
    async fn 请求(state: AppState, 令牌: Option<&str>) -> axum::response::Response {
        let app = super::super::build_router(state);
        let mut builder = Request::builder().method("GET").uri("/api/v1/settings");
        if let Some(t) = 令牌 {
            builder = builder.header("Authorization", format!("Bearer {t}"));
        }
        app.oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    /// 便捷：带可选 Bearer 令牌 PATCH 设置端点（JSON 请求体）。
    async fn 请求_patch(
        state: AppState,
        令牌: Option<&str>,
        body: serde_json::Value,
    ) -> axum::response::Response {
        请求_patch_原文(state, 令牌, &body.to_string()).await
    }

    /// 便捷：发送原始 JSON 文本，用于覆盖语法错误与反序列化拒绝。
    async fn 请求_patch_原文(
        state: AppState,
        令牌: Option<&str>,
        body: &str,
    ) -> axum::response::Response {
        let app = super::super::build_router(state);
        let mut builder = Request::builder()
            .method("PATCH")
            .uri("/api/v1/settings")
            .header("Content-Type", "application/json");
        if let Some(t) = 令牌 {
            builder = builder.header("Authorization", format!("Bearer {t}"));
        }
        app.oneshot(builder.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    /// 便捷：以指定网络代理 + 在线更新配置重建可编辑设置槽并注入 state（模拟启动期热值）。
    fn 注入设置(
        state: &mut AppState,
        proxy: crate::config::NetworkProxyConfig,
        update: &crate::config::UpdateConfig,
    ) {
        state.settings = Arc::new(
            crate::config::EditableSettings::new(proxy, std::time::Duration::from_secs(60), update)
                .unwrap(),
        );
    }

    /// 起一个最小 HTTP 正向代理：记录首个请求行并返回 200。
    async fn 启动_mock_代理() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_task = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let seen_conn = seen_task.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    if n > 0 {
                        let text = String::from_utf8_lossy(&buf[..n]);
                        if let Some(line) = text.lines().next() {
                            seen_conn.lock().unwrap().push(line.to_string());
                        }
                    }
                    let _ = sock
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                        .await;
                    let _ = sock.flush().await;
                });
            }
        });
        等待端口就绪(&addr).await;
        (addr, seen)
    }

    /// 起一个最小源站：记录是否被直连访问并返回 200。
    async fn 启动_mock_源站() -> (SocketAddr, Arc<Mutex<bool>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hit = Arc::new(Mutex::new(false));
        let hit_task = hit.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = [0u8; 512];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                if n > 0 {
                    *hit_task.lock().unwrap() = true;
                }
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await;
                let _ = sock.flush().await;
            }
        });
        (addr, hit)
    }

    async fn 等待端口就绪(addr: &SocketAddr) {
        for _ in 0..50 {
            if tokio::net::TcpStream::connect(addr).await.is_ok() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("mock 服务未就绪");
    }

    #[tokio::test]
    async fn settings_匿名被拒_401() {
        let (state, _dir) = 测试用状态().await;
        let resp = 请求(state, None).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn settings_普通用户被拒_403() {
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "u", Role::User).await;
        let resp = 请求(state, Some(&token)).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn settings_管理员成功_200_并脱敏代理凭据与隐藏_token() {
        let (mut state, _dir) = 测试用状态().await;
        // 注入含凭据的代理与更新 token（入热替换槽），断言响应中均不回显凭据
        let proxy = crate::config::NetworkProxyConfig {
            url: Some("socks5://sockuser:sockpass@socks.internal:1080".to_string()),
            http: None,
            https: None,
            all: None,
            no_proxy: Some("localhost,127.0.0.1".to_string()),
        };
        let update = crate::config::UpdateConfig {
            enabled: true,
            repo: "wcpe/JianArtifact".to_string(),
            token: Some("ghp_supersecrettoken".to_string()),
            ..Default::default()
        };
        注入设置(&mut state, proxy, &update);

        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let resp = 请求(state, Some(&token)).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = 读_json(resp).await;
        // 当前版本回显：经 build_version()（测试构建未注入环境，回退 CARGO_PKG_VERSION）
        assert_eq!(body["current_version"], crate::version::build_version());
        // 代理 URL 已脱敏：不含凭据、保留 host:port；用户名回显、has_password=true（FR-146 单代理）
        assert_eq!(
            body["network_proxy"]["proxy"]["url"],
            "socks5://socks.internal:1080"
        );
        assert_eq!(body["network_proxy"]["proxy"]["username"], "sockuser");
        assert_eq!(body["network_proxy"]["proxy"]["has_password"], true);
        assert_eq!(body["network_proxy"]["no_proxy"], "localhost,127.0.0.1");
        // 更新区：仅 has_token 布尔，绝不回显 token 本体
        assert_eq!(body["update"]["enabled"], true);
        assert_eq!(body["update"]["repo"], "wcpe/JianArtifact");
        assert_eq!(body["update"]["has_token"], true);

        // 关键脱敏断言：响应不得含任何口令明文（用户名可回显、has_password 字段名豁免）
        let text = body.to_string();
        assert!(!text.contains("sockpass"), "socks 口令不得回显：{text}");
        assert!(!text.contains(":sockpass@"), "socks 口令不得回显：{text}");
        assert!(
            !text.contains("ghp_supersecrettoken"),
            "更新 token 本体不得回显：{text}"
        );
    }

    #[tokio::test]
    async fn settings_未配置_token_时_has_token_为_false() {
        let (state, _dir) = 测试用状态().await;
        // 默认配置：update.token 为 None
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let resp = 请求(state, Some(&token)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = 读_json(resp).await;
        assert_eq!(body["update"]["has_token"], false);
    }

    // ===== PATCH /api/v1/settings 鉴权 + 热替换 + 脱敏 + 非法不改值 =====

    /// 构造一个合法的设置编辑请求体（含凭据，用于断言不回显）。
    fn 合法编辑体() -> serde_json::Value {
        serde_json::json!({
            "network_proxy": {
                "proxy": { "url": "http://new-proxy.internal:3128", "username": "user", "password": "pass" },
                "no_proxy": "localhost"
            },
            "update": {
                "enabled": true,
                "repo": "wcpe/JianArtifact",
                "api_base_url": "https://api.github.com",
                "restart_mode": "exit",
                "channel": "prerelease",
                "token": "ghp_newsecret"
            }
        })
    }

    #[tokio::test]
    async fn patch_匿名被拒_401() {
        let (state, _dir) = 测试用状态().await;
        let resp = 请求_patch(state, None, 合法编辑体()).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn patch_普通用户被拒_403() {
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "u", Role::User).await;
        let resp = 请求_patch(state, Some(&token), 合法编辑体()).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn patch_json_反序列化失败稳定返回中文_400() {
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let resp = 请求_patch_原文(state, Some(&token), r#"{"network_proxy":"#).await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = 读_json(resp).await;
        assert_eq!(body["error"]["message"], "请求体 JSON 格式非法");
    }

    #[tokio::test]
    async fn patch_json_失败时仍优先执行鉴权() {
        let (state, _dir) = 测试用状态().await;
        let anonymous = 请求_patch_原文(state.clone(), None, "{").await;
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

        let token = 签发令牌(&state, "u", Role::User).await;
        let user = 请求_patch_原文(state, Some(&token), "{").await;
        assert_eq!(user.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn patch_network_proxy_未知字段返回中文_400() {
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let body = serde_json::json!({
            "network_proxy": { "proxy": {}, "unknown": true }
        });
        let resp = 请求_patch(state, Some(&token), body).await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = 读_json(resp).await;
        assert_eq!(body["error"]["message"], "请求体 JSON 格式非法");
    }

    #[tokio::test]
    async fn patch_管理员成功_即时生效_并脱敏() {
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        // 持槽引用，PATCH 后直接断言热槽当前值已变（即时生效，无须重启）
        let settings = state.settings.clone();

        let resp = 请求_patch(state, Some(&token), 合法编辑体()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = 读_json(resp).await;
        // 响应回显脱敏后的当前值：代理去口令、用户名回显、token 仅 has_token
        assert_eq!(
            body["network_proxy"]["proxy"]["url"],
            "http://new-proxy.internal:3128"
        );
        assert_eq!(body["network_proxy"]["proxy"]["username"], "user");
        assert_eq!(body["network_proxy"]["proxy"]["has_password"], true);
        assert_eq!(body["update"]["enabled"], true);
        assert_eq!(body["update"]["restart_mode"], "exit");
        assert_eq!(body["update"]["channel"], "prerelease");
        assert_eq!(body["update"]["has_token"], true);
        // 关键脱敏：响应不含口令 / token 明文（用户名 user 可回显、has_password 字段名豁免）
        let text = body.to_string();
        assert!(!text.contains("user:pass"), "代理口令不得回显：{text}");
        assert!(!text.contains(":pass@"), "代理口令不得回显：{text}");
        assert!(!text.contains("ghp_newsecret"), "token 不得回显：{text}");

        // 热槽当前值已即时生效（PATCH 锁外重建后原子换槽）：存储 URL 含百分号编码凭据
        let snap = settings.network.snapshot();
        assert_eq!(
            snap.proxy.url.as_deref(),
            Some("http://user:pass@new-proxy.internal:3128"),
            "代理热槽应已换为新值（凭据仅入内存槽，不回显）"
        );
        let upd = settings.update();
        assert!(upd.enabled, "update.enabled 应已翻为 true");
        assert_eq!(upd.channel, "prerelease", "channel 应已热替换为 prerelease");
        assert_eq!(
            upd.token.as_deref(),
            Some("ghp_newsecret"),
            "token 应已入内存槽"
        );
    }

    #[tokio::test]
    async fn settings_管理员_get_回显默认_channel_为_stable() {
        // FR-89：默认配置 channel=stable，GET 应回显之
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let resp = 请求(state, Some(&token)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = 读_json(resp).await;
        assert_eq!(body["update"]["channel"], "stable");
    }

    #[tokio::test]
    async fn patch_非法_channel_返回_400_且不改现有生效值() {
        // FR-89：非法 channel 在 EditableUpdate::validate 阶段被拒，返回 400 且不触碰现有生效值
        let (mut state, _dir) = 测试用状态().await;
        注入设置(
            &mut state,
            crate::config::NetworkProxyConfig::default(),
            &crate::config::UpdateConfig::default(),
        );
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let settings = state.settings.clone();

        let body = serde_json::json!({
            "network_proxy": { "proxy": {}, "no_proxy": null },
            "update": {
                "enabled": true,
                "repo": "wcpe/JianArtifact",
                "api_base_url": "https://api.github.com",
                "restart_mode": "self",
                "channel": "beta",
                "token": null
            }
        });
        let resp = 请求_patch(state, Some(&token), body).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        // 非法 channel 不得改动现有生效值（仍为默认 stable、enabled=false）
        let upd = settings.update();
        assert_eq!(upd.channel, "stable", "非法 channel 不得改动现有生效通道");
        assert!(!upd.enabled, "非法配置不得改动现有 enabled");
    }

    #[tokio::test]
    async fn patch_翻_enabled_为_true_后_update_check_不再_409() {
        // FR-126 异步化：check 触发改为 POST /update/check。默认 update.enabled=false 时触发因 Disabled
        // 返回 409；PATCH 翻 true 后已过开关闸，触发返回 202（job_id），实际出站在后台任务进行
        //（出站失败 502 已移入后台 job 进度，不再是触发响应的同步错误）。本测试只验「开关闸」生效。
        let (state, _dir) = 测试用状态().await;
        let admin = 签发令牌(&state, "admin", Role::Admin).await;
        let settings = state.settings.clone();
        let meta = state.meta.clone();
        let jwt = state.jwt.clone();

        // 先确认默认触发 check 为 409（Disabled）
        let app = super::super::build_router(state);
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/update/check")
                    .header("Authorization", format!("Bearer {admin}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT, "默认未启用应 409");

        // PATCH 翻 enabled=true，指向一个不可路由的地址（不实际联网，仅验证已过 Disabled 闸）
        let _ = meta;
        let _ = jwt;
        let body = serde_json::json!({
            "network_proxy": { "proxy": {}, "no_proxy": null },
            "update": {
                "enabled": true,
                "repo": "wcpe/JianArtifact",
                "api_base_url": "http://127.0.0.1:1",
                "restart_mode": "self",
                "channel": "stable",
                "token": null
            }
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/api/v1/settings")
                    .header("Authorization", format!("Bearer {admin}"))
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(settings.update().enabled, "PATCH 后 enabled 应为 true");

        // 再次触发 check：已过 Disabled 闸，立即返回 202（job_id），不再因 Disabled 返回 409
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/update/check")
                    .header("Authorization", format!("Bearer {admin}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::CONFLICT,
            "启用后触发 check 不应再因 Disabled 返回 409"
        );
        assert_eq!(
            resp.status(),
            StatusCode::ACCEPTED,
            "启用后触发 check 应立即返回 202（job_id），出站在后台进行"
        );
    }

    #[tokio::test]
    async fn patch_非法_restart_mode_返回_400_且不改现有生效值() {
        let (mut state, _dir) = 测试用状态().await;
        // 先注入一份已知生效值
        let proxy = crate::config::NetworkProxyConfig {
            url: Some("http://old-proxy.internal:8080".to_string()),
            ..Default::default()
        };
        注入设置(&mut state, proxy, &crate::config::UpdateConfig::default());
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let settings = state.settings.clone();

        let body = serde_json::json!({
            "network_proxy": { "proxy": { "url": "http://new-proxy.internal:9999" }, "no_proxy": null },
            "update": {
                "enabled": true,
                "repo": "wcpe/JianArtifact",
                "api_base_url": "https://api.github.com",
                "restart_mode": "INVALID_MODE",
                "channel": "stable",
                "token": null
            }
        });
        let resp = 请求_patch(state, Some(&token), body).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // 非法校验先于代理换槽：现有代理与更新生效值均不变
        let snap = settings.network.snapshot();
        assert_eq!(
            snap.proxy.url.as_deref(),
            Some("http://old-proxy.internal:8080"),
            "非法配置不得改动现有生效代理"
        );
        assert!(
            !settings.update().enabled,
            "非法配置不得改动现有生效的 update（仍为默认关闭）"
        );
    }

    #[tokio::test]
    async fn patch_token_三态_缺省保留_空串清空() {
        let (mut state, _dir) = 测试用状态().await;
        // 初始已配置 token
        let update = crate::config::UpdateConfig {
            token: Some("ghp_existing".to_string()),
            ..Default::default()
        };
        注入设置(
            &mut state,
            crate::config::NetworkProxyConfig::default(),
            &update,
        );
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let settings = state.settings.clone();
        let app = super::super::build_router(state);

        // ① token 缺省：保留现有
        let body = serde_json::json!({
            "network_proxy": { "proxy": {}, "no_proxy": null },
            "update": { "enabled": false, "repo": "wcpe/JianArtifact", "api_base_url": "https://api.github.com", "restart_mode": "self", "channel": "stable" }
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/api/v1/settings")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            settings.update().token.as_deref(),
            Some("ghp_existing"),
            "token 缺省应保留现有"
        );

        // ② token 空串：清空
        let body = serde_json::json!({
            "network_proxy": { "proxy": {}, "no_proxy": null },
            "update": { "enabled": false, "repo": "wcpe/JianArtifact", "api_base_url": "https://api.github.com", "restart_mode": "self", "channel": "stable", "token": "" }
        });
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/api/v1/settings")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(settings.update().token.is_none(), "token 空串应清空");
    }

    // ===== FR-109：部分 PATCH（设置页只发 network_proxy、系统页只发 update）=====

    #[tokio::test]
    async fn 部分patch_仅update_不动代理() {
        // 系统页迁入在线更新后只发 update 块、不带 network_proxy：应成功且仅改在线更新。
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let settings = state.settings.clone();
        let app = super::super::build_router(state);

        let body = serde_json::json!({
            "update": { "enabled": true, "repo": "wcpe/JianArtifact", "api_base_url": "https://api.github.com", "restart_mode": "exit", "channel": "stable" }
        });
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/api/v1/settings")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "缺 network_proxy 块应被接受");
        assert!(settings.update().enabled, "在线更新应已更新");
        assert_eq!(settings.update().restart_mode, "exit");
    }

    #[tokio::test]
    async fn 部分patch_仅代理_不动update() {
        // 设置页移除在线更新节后只发 network_proxy 块、不带 update：应成功且不动在线更新。
        let (mut state, _dir) = 测试用状态().await;
        let update = crate::config::UpdateConfig {
            repo: "owner/keep".to_string(),
            ..Default::default()
        };
        注入设置(&mut state, NetworkProxyConfig::default(), &update);
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let settings = state.settings.clone();
        let app = super::super::build_router(state);

        let body = serde_json::json!({
            "network_proxy": { "proxy": { "url": "http://proxy.local:8080" }, "no_proxy": null }
        });
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/api/v1/settings")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "缺 update 块应被接受");
        assert_eq!(
            settings.update().repo,
            "owner/keep",
            "在线更新未提供应保持不动"
        );
        assert!(
            settings.network.snapshot().proxy.url.is_some(),
            "代理应已更新"
        );
    }

    #[tokio::test]
    async fn patch_旧三键契约返回_400_且内存与数据库不变() {
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let settings = state.settings.clone();
        let meta = state.meta.clone();

        let seed = serde_json::json!({
            "network_proxy": {
                "proxy": { "url": "http://saved-proxy.internal:3128" },
                "no_proxy": "localhost"
            }
        });
        let resp = 请求_patch(state.clone(), Some(&token), seed).await;
        assert_eq!(resp.status(), StatusCode::OK, "新契约代理应先成功保存");
        let memory_before = {
            let proxy = settings.network.snapshot().proxy.clone();
            (
                proxy.url,
                proxy.http,
                proxy.https,
                proxy.all,
                proxy.no_proxy,
            )
        };
        let db_before = meta.load_settings().await.unwrap();
        assert!(
            db_before.iter().any(|(key, _)| key == PROXY_SETTING_KEY),
            "测试前应已有非空代理落库"
        );

        let legacy = serde_json::json!({
            "network_proxy": {
                "http": { "url": "http://legacy-http.internal:8080" },
                "https": { "url": "http://legacy-https.internal:8443" },
                "all": { "url": "socks5://legacy-all.internal:1080" },
                "no_proxy": "example.invalid"
            }
        });
        let resp = 请求_patch(state, Some(&token), legacy).await;
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "旧三键 PATCH 契约必须明确拒绝"
        );
        let memory_after = {
            let proxy = settings.network.snapshot().proxy.clone();
            (
                proxy.url,
                proxy.http,
                proxy.https,
                proxy.all,
                proxy.no_proxy,
            )
        };
        assert_eq!(memory_after, memory_before, "拒绝旧契约后内存代理不得变化");
        assert_eq!(
            meta.load_settings().await.unwrap(),
            db_before,
            "拒绝旧契约后数据库代理记录不得变化"
        );
    }

    #[tokio::test]
    async fn 并发patch结束后内存代理与数据库持久值一致() {
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let settings = Arc::clone(&state.settings);
        let meta = state.meta.clone();

        let mut tasks = Vec::new();
        for i in 0..24 {
            let state = state.clone();
            let token = token.clone();
            tasks.push(tokio::spawn(async move {
                let body = serde_json::json!({
                    "network_proxy": {
                        "proxy": { "url": format!("http://proxy-{i}.internal:3128") },
                        "no_proxy": format!("bypass-{i}.internal")
                    }
                });
                请求_patch(state, Some(&token), body).await.status()
            }));
        }
        for task in tasks {
            assert_eq!(task.await.unwrap(), StatusCode::OK);
        }

        let memory = settings.network.snapshot().proxy.clone();
        let (_, json) = meta
            .load_settings()
            .await
            .unwrap()
            .into_iter()
            .find(|(key, _)| key == PROXY_SETTING_KEY)
            .expect("并发 PATCH 后应有代理持久值");
        let key = state.jwt.derive_key(crate::crypto_box::PROXY_KEY_DOMAIN);
        let persisted = restore_proxy_from_db(&json, &key).expect("持久值应可恢复");
        assert_eq!(
            persisted.effective_url(),
            memory.effective_url(),
            "并发 PATCH 完成后 DB 代理必须与运行时代理一致"
        );
        assert_eq!(
            persisted.no_proxy, memory.no_proxy,
            "并发 PATCH 完成后 DB no_proxy 必须与运行时一致"
        );
    }

    // ===== FR-100：rebuild_proxy_url 纯函数穷举 =====

    /// 便捷：构造单代理编辑项。
    fn 编辑项(
        url: Option<&str>,
        username: Option<&str>,
        password: Option<&str>,
    ) -> ProxyEntryPatch {
        ProxyEntryPatch {
            url: url.map(str::to_string),
            username: username.map(str::to_string),
            password: password.map(str::to_string),
        }
    }

    #[test]
    fn rebuild_url_空白或缺省即清除() {
        // 规则 1：url 缺省 / 空 / 纯空白 → None（用户名 / 密码忽略）
        assert_eq!(
            rebuild_proxy_url(&编辑项(None, Some("u"), Some("p")), None),
            None
        );
        assert_eq!(
            rebuild_proxy_url(&编辑项(Some(""), Some("u"), None), None),
            None
        );
        assert_eq!(
            rebuild_proxy_url(&编辑项(Some("   "), None, None), None),
            None
        );
    }

    #[test]
    fn rebuild_url_无用户名即纯_host_无_userinfo() {
        // 规则 5：用户名为空 → 直接 host，即便给了密码也忽略（无用户不能单挂密码）
        assert_eq!(
            rebuild_proxy_url(&编辑项(Some("http://host:3128"), None, Some("p")), None),
            Some("http://host:3128".to_string())
        );
        assert_eq!(
            rebuild_proxy_url(
                &编辑项(Some("http://host:3128"), Some("  "), Some("p")),
                None
            ),
            Some("http://host:3128".to_string())
        );
    }

    #[test]
    fn rebuild_url_用户名加密码组装_userinfo() {
        // 规则 5：username + password → scheme://user:pass@host
        assert_eq!(
            rebuild_proxy_url(
                &编辑项(Some("http://host:3128"), Some("alice"), Some("secret")),
                None
            ),
            Some("http://alice:secret@host:3128".to_string())
        );
    }

    #[test]
    fn rebuild_url_仅用户名无密码() {
        // 规则 5：username 非空、password 空串 → 仅 user@（无密码）
        assert_eq!(
            rebuild_proxy_url(
                &编辑项(Some("http://host:3128"), Some("alice"), Some("")),
                None
            ),
            Some("http://alice@host:3128".to_string())
        );
    }

    #[test]
    fn rebuild_url_密码缺省_同用户名_保留现有密码() {
        // 规则 4：password None + username 与 current 一致 → 沿用现有密码
        let current = "http://alice:oldpass@host:3128";
        assert_eq!(
            rebuild_proxy_url(
                &编辑项(Some("http://host:3128"), Some("alice"), None),
                Some(current)
            ),
            Some("http://alice:oldpass@host:3128".to_string())
        );
    }

    #[test]
    fn rebuild_url_密码缺省_改用户名_不保留现有密码() {
        // 规则 4：password None + username 与 current 不一致 → 视为无密码
        let current = "http://alice:oldpass@host:3128";
        assert_eq!(
            rebuild_proxy_url(
                &编辑项(Some("http://host:3128"), Some("bob"), None),
                Some(current)
            ),
            Some("http://bob@host:3128".to_string())
        );
    }

    #[test]
    fn rebuild_url_密码空串_清空现有密码() {
        // 规则 4：password "" → 清空，即便 current 同用户名有密码
        let current = "http://alice:oldpass@host:3128";
        assert_eq!(
            rebuild_proxy_url(
                &编辑项(Some("http://host:3128"), Some("alice"), Some("")),
                Some(current)
            ),
            Some("http://alice@host:3128".to_string())
        );
    }

    #[test]
    fn rebuild_url_用户误带凭据被剥离后重组() {
        // 规则 2：url 含 userinfo 被 sanitize 剥离，再据 username/password 字段重组
        assert_eq!(
            rebuild_proxy_url(
                &编辑项(
                    Some("http://wrong:wrong@host:3128"),
                    Some("alice"),
                    Some("secret")
                ),
                None
            ),
            Some("http://alice:secret@host:3128".to_string())
        );
    }

    #[test]
    fn rebuild_url_socks5_scheme() {
        // 规则 5：socks5 scheme 同样组装 userinfo
        assert_eq!(
            rebuild_proxy_url(
                &编辑项(Some("socks5://host:1080"), Some("alice"), Some("secret")),
                None
            ),
            Some("socks5://alice:secret@host:1080".to_string())
        );
    }

    #[test]
    fn rebuild_url_口令含特殊字符百分号编码() {
        // 规则 5：口令含 `@` `:` `/` 等保留字符 → 百分号编码，避免重组歧义
        let out = rebuild_proxy_url(
            &编辑项(Some("http://host:3128"), Some("al@ice"), Some("p@ss:w/ord")),
            None,
        )
        .unwrap();
        // 用户名 / 口令中的保留字符被编码：@→%40 : →%3A / →%2F
        assert_eq!(out, "http://al%40ice:p%40ss%3Aw%2Ford@host:3128");
        // 重组后用脱敏 + 解析能还原回显用户名（往返一致）
        let (user, has_pw) = parse_proxy_credentials(&out);
        assert_eq!(user.as_deref(), Some("al@ice"));
        assert!(has_pw);
    }

    #[test]
    fn parse_credentials_含_at_口令的脱敏与解析() {
        // 口令含 `@`（百分号编码为 %40）：解析回显用户名正确、has_password 为真
        let stored = "http://alice:p%40ss@host:3128";
        let (user, has_pw) = parse_proxy_credentials(stored);
        assert_eq!(user.as_deref(), Some("alice"));
        assert!(has_pw);
        // 脱敏只留 host:port，不含任何凭据
        assert_eq!(sanitize_proxy_url(stored), "http://host:3128");
    }

    #[test]
    fn parse_credentials_无_userinfo_或仅密码() {
        // 无 userinfo → 无回显、无密码
        assert_eq!(parse_proxy_credentials("http://host:3128"), (None, false));
        // 仅用户名无密码 → 回显用户名、has_password=false
        let (u, p) = parse_proxy_credentials("http://alice@host:3128");
        assert_eq!(u.as_deref(), Some("alice"));
        assert!(!p);
    }

    // ===== FR-106：在线更新非密钥字段落库 + 凭据红线 =====

    #[tokio::test]
    async fn patch_settings_落库_update_非密钥字段_重装载仍生效() {
        // FR-106：PATCH update 非密钥字段 → 写 app_settings(key=update) → 重新装载后仍生效
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let meta = state.meta.clone();

        let body = serde_json::json!({
            "network_proxy": { "proxy": {}, "no_proxy": null },
            "update": {
                "enabled": true,
                "repo": "acme/app",
                "api_base_url": "https://api.github.com",
                "restart_mode": "exit",
                "channel": "prerelease",
                "token": "ghp_secret_should_not_persist"
            }
        });
        let resp = 请求_patch(state, Some(&token), body).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let rows = meta.load_settings().await.unwrap();
        // update 节已落库（非密钥字段）
        assert!(
            rows.iter().any(|(k, _)| k == "update"),
            "update 非密钥字段应已落库"
        );
        // 重新装载：经覆盖纯函数合并，enabled / repo / channel 仍生效（token 走 env / 文件，不在 DB）
        let eff = crate::config_overlay::merge_effective_config(
            crate::config::Config::default(),
            &std::collections::BTreeSet::new(),
            &rows,
        );
        assert!(eff.update.enabled, "重新装载后 enabled 应仍为 true");
        assert_eq!(eff.update.repo, "acme/app");
        assert_eq!(eff.update.channel, "prerelease");
        assert_eq!(eff.update.restart_mode, "exit");
        // token 绝不入库：重新装载回落文件默认 None（真源是 env，不在 app_settings）
        assert!(
            eff.update.token.is_none(),
            "token 绝不入库，重装载应回落 None"
        );
    }

    #[tokio::test]
    async fn patch_settings_凭据红线_token_不入库_代理密码仅密文落库() {
        // ADR-0030 红线：PATCH 含 update token + 代理账密 →
        // - update token 绝不入库（真源 env）；
        // - 代理密码**只以密文**落 network.proxy（明文密码绝不入库）；
        // - 代理 URL（脱敏 host）与用户名（标识、非密钥）明文落库 OK。
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let meta = state.meta.clone();
        let jwt = state.jwt.clone();

        let body = serde_json::json!({
            "network_proxy": {
                "proxy": { "url": "http://proxy.internal:3128", "username": "alice", "password": "supersecretpw" },
                "no_proxy": null
            },
            "update": {
                "enabled": true,
                "repo": "wcpe/JianArtifact",
                "api_base_url": "https://api.github.com",
                "restart_mode": "self",
                "channel": "stable",
                "token": "ghp_topsecrettoken"
            }
        });
        let resp = 请求_patch(state, Some(&token), body).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let rows = meta.load_settings().await.unwrap();
        let all: String = rows.iter().map(|(k, v)| format!("{k}{v}")).collect();
        // update token 本体绝不入库（真源 env）
        assert!(
            !all.contains("ghp_topsecrettoken"),
            "update token 绝不入库：{all}"
        );
        // 代理密码明文绝不入库（只落密文）
        assert!(
            !all.contains("supersecretpw"),
            "代理密码明文绝不入库（仅密文）：{all}"
        );

        // 代理节已落库为加密形态（ADR-0030）：network.proxy 键存在
        let proxy_row = rows
            .iter()
            .find(|(k, _)| k == PROXY_SETTING_KEY)
            .expect("network.proxy 应已落库");
        let persisted: PersistedProxy = serde_json::from_str(&proxy_row.1).unwrap();
        let entry = persisted.proxy.expect("代理应已按单项形态落库（FR-146）");
        assert!(persisted.http.is_none(), "新写入不得再落旧 http 项");
        // URL 脱敏 host + 用户名明文落库（均非密钥）
        assert_eq!(entry.url.as_deref(), Some("http://proxy.internal:3128"));
        assert_eq!(entry.username.as_deref(), Some("alice"));
        // 密码以密文落库（password_enc 非空、且不等于明文）
        let enc = entry.password_enc.expect("密码应以密文落库");
        assert_ne!(enc, "supersecretpw", "落库密码不得为明文");
        assert!(!enc.contains("supersecretpw"), "密文不得含明文子串：{enc}");
        // 用派生子密钥能解密回明文（重启恢复的等价路径）
        let key = jwt.derive_key(crate::crypto_box::PROXY_KEY_DOMAIN);
        assert_eq!(
            crate::crypto_box::decrypt_secret(&key, &enc).as_deref(),
            Some("supersecretpw"),
            "密文应能用派生子密钥解回明文"
        );
    }

    #[tokio::test]
    async fn patch_settings_代理落库后重启恢复含密码() {
        // ADR-0030：PATCH 代理（带密码）→ 落库 → 走启动恢复纯函数 → 代理含密码仍在（重启不丢）
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let meta = state.meta.clone();
        let jwt = state.jwt.clone();

        let body = serde_json::json!({
            "network_proxy": {
                "proxy": { "url": "socks5://socks.internal:1080", "username": "su", "password": "sp" },
                "no_proxy": "localhost,127.0.0.1"
            },
            "update": {
                "enabled": false,
                "repo": "wcpe/JianArtifact",
                "api_base_url": "https://api.github.com",
                "restart_mode": "self",
                "channel": "stable",
                "token": null
            }
        });
        let resp = 请求_patch(state, Some(&token), body).await;
        assert_eq!(resp.status(), StatusCode::OK);

        // 取落库 JSON，走启动恢复纯函数（等价重启装配）
        let rows = meta.load_settings().await.unwrap();
        let (_, json) = rows
            .iter()
            .find(|(k, _)| k == PROXY_SETTING_KEY)
            .expect("network.proxy 应已落库");
        let key = jwt.derive_key(crate::crypto_box::PROXY_KEY_DOMAIN);
        let restored = restore_proxy_from_db(json, &key).expect("应恢复出代理");
        // 恢复出的代理 URL 含解密后的凭据（与 PATCH 时一致），密码仍在
        assert_eq!(
            restored.url.as_deref(),
            Some("socks5://su:sp@socks.internal:1080")
        );
        assert_eq!(restored.no_proxy.as_deref(), Some("localhost,127.0.0.1"));
    }

    #[tokio::test]
    async fn 旧三项落库_json_恢复时按优先级折叠为单代理() {
        // FR-146 向后兼容：升级前落库的旧三项形态（http/https/all）恢复时按 all > https > http
        // 折叠为单代理，凭据不丢（用真实加密密文构造旧形态 JSON）。
        let (state, _dir) = 测试用状态().await;
        let key = state.jwt.derive_key(crate::crypto_box::PROXY_KEY_DOMAIN);
        let http_enc = crate::crypto_box::encrypt_secret(&key, "hp");
        let all_enc = crate::crypto_box::encrypt_secret(&key, "sp");
        let legacy_json = serde_json::json!({
            "http": { "url": "http://proxy.internal:3128", "username": "alice", "password_enc": http_enc },
            "all": { "url": "socks5://socks.internal:1080", "username": "su", "password_enc": all_enc },
            "no_proxy": "localhost"
        })
        .to_string();
        let restored = restore_proxy_from_db(&legacy_json, &key).expect("旧形态应能恢复");
        // all 优先于 http：单代理取 all 项，凭据解密回明文
        assert_eq!(
            restored.url.as_deref(),
            Some("socks5://su:sp@socks.internal:1080")
        );
        assert_eq!(restored.no_proxy.as_deref(), Some("localhost"));
    }

    #[tokio::test]
    async fn patch_settings_空代理删键_恢复回落文件默认() {
        // ADR-0030：先落一份代理，再 PATCH 空代理 → 删 network.proxy 键 → 恢复返回 None（回落文件默认）
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let meta = state.meta.clone();

        let 带代理 = serde_json::json!({
            "network_proxy": { "proxy": { "url": "http://p.internal:3128" }, "no_proxy": null },
            "update": { "enabled": false, "repo": "wcpe/JianArtifact", "api_base_url": "https://api.github.com", "restart_mode": "self", "channel": "stable", "token": null }
        });
        let resp = 请求_patch(state.clone(), Some(&token), 带代理).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            meta.load_settings()
                .await
                .unwrap()
                .iter()
                .any(|(k, _)| k == PROXY_SETTING_KEY),
            "应已落库一份代理"
        );

        let 空代理 = serde_json::json!({
            "network_proxy": { "proxy": {}, "no_proxy": null },
            "update": { "enabled": false, "repo": "wcpe/JianArtifact", "api_base_url": "https://api.github.com", "restart_mode": "self", "channel": "stable", "token": null }
        });
        let resp = 请求_patch(state, Some(&token), 空代理).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            !meta
                .load_settings()
                .await
                .unwrap()
                .iter()
                .any(|(k, _)| k == PROXY_SETTING_KEY),
            "空代理应删 network.proxy 键，恢复回落文件默认"
        );
    }

    #[tokio::test]
    async fn patch_settings_校验失败_400_不落库() {
        // FR-106：非法 channel 返回 400，app_settings 不被写入 update 节
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let meta = state.meta.clone();
        let body = serde_json::json!({
            "network_proxy": { "proxy": {}, "no_proxy": null },
            "update": {
                "enabled": true,
                "repo": "wcpe/JianArtifact",
                "api_base_url": "https://api.github.com",
                "restart_mode": "self",
                "channel": "beta",
                "token": null
            }
        });
        let resp = 请求_patch(state, Some(&token), body).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(
            meta.load_settings().await.unwrap().is_empty(),
            "校验失败不得落库 update 节"
        );
    }

    // ===== FR-128：proxy_test 端点鉴权 + URL 校验 + 连通性测试 =====

    /// 便捷：带可选 Bearer 令牌 POST proxy-test 端点（JSON 请求体）。
    async fn 请求_proxy_test(
        state: AppState,
        令牌: Option<&str>,
        body: serde_json::Value,
    ) -> axum::response::Response {
        let app = super::super::build_router(state);
        let mut builder = Request::builder()
            .method("POST")
            .uri("/api/v1/settings/proxy-test")
            .header("Content-Type", "application/json");
        if let Some(t) = 令牌 {
            builder = builder.header("Authorization", format!("Bearer {t}"));
        }
        app.oneshot(builder.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn proxy_test_匿名被拒_401() {
        let (state, _dir) = 测试用状态().await;
        let resp = 请求_proxy_test(
            state,
            None,
            serde_json::json!({"url": "https://example.com"}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn proxy_test_普通用户被拒_403() {
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "u", Role::User).await;
        let resp = 请求_proxy_test(
            state,
            Some(&token),
            serde_json::json!({"url": "https://example.com"}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn proxy_test_非_http_scheme_返回_400() {
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        // ftp:// 非法
        let resp = 请求_proxy_test(
            state.clone(),
            Some(&token),
            serde_json::json!({"url": "ftp://example.com/file"}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn proxy_test_空串返回_400() {
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let resp = 请求_proxy_test(state, Some(&token), serde_json::json!({"url": ""})).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn proxy_test_file_scheme_返回_400() {
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let resp = 请求_proxy_test(
            state,
            Some(&token),
            serde_json::json!({"url": "file:///etc/passwd"}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn proxy_test_不可达地址返回_ok_false() {
        // 对 127.0.0.1:1（无监听）发 GET，期望返回 200 OK（端点本身成功）但内含 ok=false（连通性失败）
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let resp = 请求_proxy_test(
            state,
            Some(&token),
            serde_json::json!({"url": "http://127.0.0.1:1"}),
        )
        .await;
        // 端点本身正常响应 200；但 body.ok = false（连接失败）
        assert_eq!(resp.status(), StatusCode::OK);
        let body = 读_json(resp).await;
        assert_eq!(body["ok"], false, "连接失败应为 ok=false");
        assert!(body["error"].is_string(), "连接失败应有 error 字段：{body}");
        assert!(
            body.get("elapsed_ms").is_some(),
            "应含 elapsed_ms 字段：{body}"
        );
    }

    #[tokio::test]
    async fn proxy_test_https_合法_url_不可达返回_ok_false() {
        // https 前缀合法，但地址不可达 → ok=false
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let resp = 请求_proxy_test(
            state,
            Some(&token),
            serde_json::json!({"url": "https://127.0.0.1:1"}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = 读_json(resp).await;
        assert_eq!(body["ok"], false);
    }

    #[tokio::test]
    async fn proxy_test_malformed_http_https_url_返回_400() {
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;

        for url in ["http://", "https://"] {
            let resp =
                请求_proxy_test(state.clone(), Some(&token), serde_json::json!({"url": url})).await;
            assert_eq!(
                resp.status(),
                StatusCode::BAD_REQUEST,
                "格式损坏的目标 URL 必须返回 400：{url}"
            );
        }
    }

    #[tokio::test]
    async fn proxy_test_表单代理临时生效_无需保存() {
        let (proxy_addr, seen) = 启动_mock_代理().await;
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;

        let resp = 请求_proxy_test(
            state,
            Some(&token),
            serde_json::json!({
                "url": "http://example.invalid/probe",
                "proxy": { "url": format!("http://{proxy_addr}") }
            }),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = 读_json(resp).await;
        assert_eq!(body["ok"], true, "mock 代理返回 200，测试应成功：{body}");
        assert_eq!(body["status"], 200);
        let lines = seen.lock().unwrap().clone();
        assert!(
            lines
                .iter()
                .any(|l| l.contains("http://example.invalid/probe")),
            "表单代理应收到目标 URL 的 absolute-form 请求行，实际：{lines:?}"
        );
    }

    #[tokio::test]
    async fn proxy_test_显式表单代理不继承已保存_no_proxy() {
        let (form_proxy_addr, form_seen) = 启动_mock_代理().await;
        let (origin_addr, origin_hit) = 启动_mock_源站().await;
        let (mut state, _dir) = 测试用状态().await;
        let saved = crate::config::NetworkProxyConfig {
            url: Some("http://127.0.0.1:1".to_string()),
            no_proxy: Some(origin_addr.ip().to_string()),
            ..Default::default()
        };
        注入设置(&mut state, saved, &crate::config::UpdateConfig::default());
        let token = 签发令牌(&state, "admin", Role::Admin).await;
        let target = format!("http://{origin_addr}/must-use-form-proxy");

        let resp = 请求_proxy_test(
            state,
            Some(&token),
            serde_json::json!({
                "url": target,
                "proxy": { "url": format!("http://{form_proxy_addr}") }
            }),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = 读_json(resp).await;
        assert_eq!(
            body["ok"], true,
            "显式表单代理返回 200 时测试应成功：{body}"
        );
        let lines = form_seen.lock().unwrap().clone();
        assert!(
            lines.iter().any(|line| line.contains(&target)),
            "显式表单代理必须收到请求，不得继承已保存 no_proxy：{lines:?}"
        );
        assert!(
            !*origin_hit.lock().unwrap(),
            "显式表单代理存在时不应因已保存 no_proxy 直连源站"
        );
    }

    #[tokio::test]
    async fn proxy_test_缺省_proxy_仍使用已保存代理() {
        let (proxy_addr, seen) = 启动_mock_代理().await;
        let (mut state, _dir) = 测试用状态().await;
        let proxy = crate::config::NetworkProxyConfig {
            url: Some(format!("http://{proxy_addr}")),
            ..Default::default()
        };
        注入设置(&mut state, proxy, &crate::config::UpdateConfig::default());
        let token = 签发令牌(&state, "admin", Role::Admin).await;

        let resp = 请求_proxy_test(
            state,
            Some(&token),
            serde_json::json!({"url": "http://example.invalid/saved"}),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = 读_json(resp).await;
        assert_eq!(body["ok"], true, "缺省 proxy 应沿用已保存代理：{body}");
        let lines = seen.lock().unwrap().clone();
        assert!(
            lines
                .iter()
                .any(|l| l.contains("http://example.invalid/saved")),
            "缺省 proxy 字段时已保存代理应继续生效，实际：{lines:?}"
        );
    }

    #[tokio::test]
    async fn proxy_test_空表单代理强制直连_不走已保存代理() {
        let (proxy_addr, seen) = 启动_mock_代理().await;
        let (origin_addr, origin_hit) = 启动_mock_源站().await;
        let (mut state, _dir) = 测试用状态().await;
        let proxy = crate::config::NetworkProxyConfig {
            url: Some(format!("http://{proxy_addr}")),
            ..Default::default()
        };
        注入设置(&mut state, proxy, &crate::config::UpdateConfig::default());
        let token = 签发令牌(&state, "admin", Role::Admin).await;

        let resp = 请求_proxy_test(
            state,
            Some(&token),
            serde_json::json!({
                "url": format!("http://{origin_addr}/direct"),
                "proxy": { "url": "" }
            }),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = 读_json(resp).await;
        assert_eq!(body["ok"], true, "显式空代理应直连源站：{body}");
        assert!(*origin_hit.lock().unwrap(), "显式空代理应直连命中源站");
        let lines = seen.lock().unwrap().clone();
        assert!(lines.is_empty(), "显式空代理不应走已保存代理：{lines:?}");
    }

    #[tokio::test]
    async fn proxy_test_表单代理非法_scheme_返回_400_且不泄露凭据() {
        let (state, _dir) = 测试用状态().await;
        let token = 签发令牌(&state, "admin", Role::Admin).await;

        let resp = 请求_proxy_test(
            state,
            Some(&token),
            serde_json::json!({
                "url": "http://example.invalid/probe",
                "proxy": { "url": "ftp://secretuser:secretpass@proxy.internal:21" }
            }),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = 读_json(resp).await;
        let message = body["error"]["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("代理配置非法"),
            "应返回中文代理配置错误：{body}"
        );
        assert!(
            !message.contains("secretuser") && !message.contains("secretpass"),
            "错误信息不得泄露代理凭据：{message}"
        );
    }
}
