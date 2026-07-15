//! 严格编码解码领域原语。

/// 严格解码 RFC 4648 标准 Base64，接受规范填充和无填充形式。
pub fn strict_base64_decode(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    let data_len = base64_data_len(bytes)?;
    let data = &bytes[..data_len];
    if !tail_bits_are_zero(data) {
        return None;
    }
    decode_base64_data(data)
}

fn decode_base64_data(data: &[u8]) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(data.len().saturating_mul(3) / 4);
    for chunk in data.chunks(4) {
        let first = base64_sextet(*chunk.first()?)?;
        let second = base64_sextet(*chunk.get(1)?)?;
        output.push((first << 2) | (second >> 4));
        if let Some(&third) = chunk.get(2) {
            let third = base64_sextet(third)?;
            output.push((second << 4) | (third >> 2));
            if let Some(&fourth) = chunk.get(3) {
                output.push((third << 6) | base64_sextet(fourth)?);
            }
        }
    }
    Some(output)
}

fn base64_data_len(input: &[u8]) -> Option<usize> {
    let padding = input.iter().rev().take_while(|&&byte| byte == b'=').count();
    if padding > 2 {
        return None;
    }
    let data_len = input.len().checked_sub(padding)?;
    if input[..data_len].contains(&b'=') {
        return None;
    }
    match padding {
        0 if data_len % 4 != 1 => Some(data_len),
        1 if input.len().is_multiple_of(4) && data_len % 4 == 3 => Some(data_len),
        2 if input.len().is_multiple_of(4) && data_len % 4 == 2 => Some(data_len),
        _ => None,
    }
}

fn tail_bits_are_zero(data: &[u8]) -> bool {
    let Some(&last) = data.last() else {
        return true;
    };
    let Some(last) = base64_sextet(last) else {
        return false;
    };
    match data.len() % 4 {
        2 => last & 0b0000_1111 == 0,
        3 => last & 0b0000_0011 == 0,
        _ => true,
    }
}

fn base64_sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::strict_base64_decode;

    #[test]
    fn 严格base64接受标准填充与无填充形式() {
        assert_eq!(strict_base64_decode("dXNlcjpwYXNz").unwrap(), b"user:pass");
        assert_eq!(strict_base64_decode("YWRtaW46cHc=").unwrap(), b"admin:pw");
        assert_eq!(strict_base64_decode("Zg==").unwrap(), b"f");
        assert_eq!(strict_base64_decode("Zg").unwrap(), b"f");
        assert_eq!(strict_base64_decode("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn 严格base64拒绝非法填充字符长度与尾部位() {
        for invalid in [
            "=AAA", "A=AA", "AA=A", "AA==A", "AAA==", "AAAA=", "AAAA==", "****", "A", "AAAA A",
            " Zg==", "Zg== ", "AB==", "AB", "AAB=", "AAB",
        ] {
            assert!(
                strict_base64_decode(invalid).is_none(),
                "应拒绝 {invalid:?}"
            );
        }
    }
}
