//! 制品对象路径值对象：在进入仓储与 blob 层前统一拒绝歧义或可逃逸路径。

use std::fmt;

const MAX_PATH_BYTES: usize = 4096;

/// 制品对象路径校验错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("制品路径无效")]
pub struct ArtifactPathError;

/// 指向单个制品的规范化对象路径。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArtifactPath(String);

impl ArtifactPath {
    /// 校验并构造对象路径。
    pub fn parse(value: &str) -> Result<Self, ArtifactPathError> {
        validate_path(value, false)?;
        Ok(Self(value.to_string()))
    }

    /// 校验原始 URI 路径片段中不存在危险的百分号编码。
    pub fn validate_uri_encoding(value: &str) -> Result<(), ArtifactPathError> {
        if has_encoded_escape(value) {
            Err(ArtifactPathError)
        } else {
            Ok(())
        }
    }

    /// 返回规范化路径文本。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ArtifactPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// 用于目录列表的规范化前缀；空字符串代表仓库根目录。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArtifactDirectory(String);

impl ArtifactDirectory {
    /// 校验目录并移除可选末尾斜杠。
    pub fn parse(value: &str) -> Result<Self, ArtifactPathError> {
        if value.is_empty() {
            return Ok(Self(String::new()));
        }
        if value.starts_with('/') {
            return Err(ArtifactPathError);
        }
        let normalized = value.strip_suffix('/').unwrap_or(value);
        validate_path(normalized, false)?;
        Ok(Self(normalized.to_string()))
    }

    /// 返回不带末尾斜杠的目录前缀。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn validate_path(value: &str, allow_empty: bool) -> Result<(), ArtifactPathError> {
    if (!allow_empty && value.is_empty())
        || value.len() > MAX_PATH_BYTES
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains('\\')
        || value.chars().any(char::is_control)
        || is_drive_path(value)
        || has_encoded_escape(value)
    {
        return Err(ArtifactPathError);
    }
    if value
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(ArtifactPathError);
    }
    Ok(())
}

fn is_drive_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn has_encoded_escape(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.windows(3).any(|window| {
        if window[0] != b'%' {
            return false;
        }
        let Some(high) = hex_value(window[1]) else {
            return false;
        };
        let Some(low) = hex_value(window[2]) else {
            return false;
        };
        matches!((high << 4) | low, 0..=31 | 37 | 46 | 47 | 58 | 92 | 127)
    })
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn 对象路径接受普通多段路径() {
        assert_eq!(
            ArtifactPath::parse("a/b/file.bin").unwrap().as_str(),
            "a/b/file.bin"
        );
        assert_eq!(
            ArtifactPath::parse("制品/版本.bin").unwrap().as_str(),
            "制品/版本.bin"
        );
    }

    #[test]
    fn 对象路径拒绝空值边界斜杠与危险段() {
        for value in ["", "/a", "a/", "a//b", ".", "..", "a/./b", "a/../b"] {
            assert!(ArtifactPath::parse(value).is_err(), "应拒绝 {value:?}");
        }
    }

    #[test]
    fn 对象路径拒绝反斜杠绝对盘符与控制字符() {
        for value in [
            r"a\b",
            r"C:\a",
            "C:a",
            r"\\server\share",
            "a\n/b",
            "a\u{7f}b",
        ] {
            assert!(ArtifactPath::parse(value).is_err(), "应拒绝 {value:?}");
        }
    }

    #[test]
    fn 对象路径拒绝编码绕过() {
        for value in [
            "%2e%2e/a",
            "a/%2F/b",
            "a/%5c/b",
            "%252e%252e/a",
            "a/%00/b",
            "C%3a/a",
        ] {
            assert!(ArtifactPath::parse(value).is_err(), "应拒绝 {value:?}");
        }
    }

    #[test]
    fn 原始uri编码拒绝保留字符但允许普通utf8编码() {
        for value in ["a%2Fb", "%2e%2e/a", "a%255cb"] {
            assert!(ArtifactPath::validate_uri_encoding(value).is_err());
        }
        assert!(ArtifactPath::validate_uri_encoding("%E5%88%B6%E5%93%81.bin").is_ok());
    }

    #[test]
    fn 对象路径按utf8字节限制四千零九十六() {
        assert!(ArtifactPath::parse(&"a".repeat(4096)).is_ok());
        assert!(ArtifactPath::parse(&"界".repeat(1366)).is_err());
    }

    #[test]
    fn 目录允许空根与单个末尾斜杠并输出无尾斜杠() {
        assert_eq!(ArtifactDirectory::parse("").unwrap().as_str(), "");
        assert_eq!(ArtifactDirectory::parse("a/b/").unwrap().as_str(), "a/b");
        for value in ["/", "/a", "a//", "a//b", "a/../b", r"a\b", "%2e%2e/a"] {
            assert!(ArtifactDirectory::parse(value).is_err(), "应拒绝 {value:?}");
        }
    }
}
