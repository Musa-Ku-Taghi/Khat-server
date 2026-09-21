pub fn luhn_check(digits: &str) -> bool {
    let mut sum = 0u32;
    let mut alternate = false;
    for ch in digits.chars().rev() {
        let Some(mut n) = ch.to_digit(10) else {
            return false;
        };
        if alternate {
            n *= 2;
            if n > 9 {
                n -= 9;
            }
        }
        sum += n;
        alternate = !alternate;
    }
    sum % 10 == 0
}

pub fn validate_iccid(iccid: &str, allowed_ccs: &[String]) -> Result<String, String> {
    let len = iccid.len();
    if len != 19 && len != 20 {
        return Err("ICCID must be 19 or 20 digits".to_string());
    }
    if !iccid.chars().all(|c| c.is_ascii_digit()) {
        return Err("ICCID must contain only digits".to_string());
    }
    if !luhn_check(iccid) {
        return Err("ICCID checksum failed".to_string());
    }
    if !iccid.starts_with("89") {
        return Err("ICCID must start with '89'".to_string());
    }

    let rest = &iccid[2..];
    for len in 1..=3 {
        if rest.len() >= len {
            let cc = &rest[0..len];
            if allowed_ccs.iter().any(|allowed| allowed == cc) {
                return Ok(cc.to_string());
            }
        }
    }
    Err("Country code not allowed".to_string())
}

pub fn validate_file_hash(hash: &str) -> Result<(), String> {
    if hash.len() != 64 {
        return Err("Hash must be exactly 64 hex characters".to_string());
    }
    if !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Hash must contain only hex digits".to_string());
    }
    Ok(())
}

fn is_valid_hex_color(s: &str) -> bool {
    matches!(s.len(), 3 | 4 | 6 | 8) && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn validate_link(s: &str) -> Result<String, String> {
    if s.is_empty() {
        return Err("empty link".to_string());
    }
    if s.len() > 2048 {
        return Err(format!("link too long: {} > 2048", s.len()));
    }

    let has_scheme = s.find(':').is_some_and(|colon_pos| {
        let scheme = &s[..colon_pos];
        !scheme.is_empty()
            && scheme
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });

    let candidate = if has_scheme {
        s.to_string()
    } else {
        format!("https://{s}")
    };

    let parsed = url::Url::parse(&candidate).map_err(|e| format!("invalid link: {e}"))?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("link scheme not allowed: {other}")),
    }

    if parsed.host_str().is_none_or(|h| h.is_empty()) {
        return Err("link must have a host".to_string());
    }

    Ok(parsed.to_string())
}

pub fn sanitize_mark_down(md: Vec<(u64, u64, String)>) -> Result<Vec<(u64, u64, String)>, String> {
    let mut result = Vec::with_capacity(md.len());
    for (start, end, kind) in md {
        if start >= end {
            return Err(format!(
                "invalid markdown range: start {start} must be < end {end}"
            ));
        }
        match kind.as_str() {
            "b" | "i" | "u" | "s" | "sub" | "sup" => result.push((start, end, kind)),
            _ => {
                if let Some(hex) = kind.strip_prefix("c#") {
                    if is_valid_hex_color(hex) {
                        result.push((start, end, kind));
                    }
                } else if let Some(hex) = kind.strip_prefix("h#") {
                    if is_valid_hex_color(hex) {
                        result.push((start, end, kind));
                    }
                } else if let Some(link) = kind.strip_prefix('l') {
                    if let Ok(normalized) = validate_link(link) {
                        result.push((start, end, format!("l{normalized}")));
                    }
                } else {
                    return Err(format!("unknown markdown type: {kind}"));
                }
            }
        }
    }
    Ok(result)
}
