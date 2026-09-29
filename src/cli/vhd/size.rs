//! Byte counts on the command line: `4096`, `0x1000`, `64K`, `2MiB`, `1G`.

/// A byte count: decimal or `0x` hex, with an optional binary suffix
/// (`K`, `M`, `G`, `T`, each optionally followed by `iB` or `B`, any
/// case). `1K` is 1024. A value that overflows 64 bits is refused rather
/// than wrapped.
pub fn parse(text: &str) -> Result<u64, String> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).map_err(|e| format!("{text:?}: {e}"));
    }
    let digits_end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, suffix) = text.split_at(digits_end);
    if digits.is_empty() {
        return Err(format!(
            "{text:?} is not a byte count (e.g. 4096, 64K, 2M, 1G)"
        ));
    }
    let n: u64 = digits.parse().map_err(|e| format!("{text:?}: {e}"))?;
    let shift = match suffix.to_ascii_lowercase().as_str() {
        "" | "b" => 0,
        "k" | "kb" | "kib" => 10,
        "m" | "mb" | "mib" => 20,
        "g" | "gb" | "gib" => 30,
        "t" | "tb" | "tib" => 40,
        _ => {
            return Err(format!(
                "{text:?}: unknown suffix {suffix:?}; use K, M, G or T (powers of 1024)"
            ))
        }
    };
    n.checked_mul(1u64 << shift)
        .ok_or_else(|| format!("{text:?} does not fit in 64 bits"))
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn plain_hex_and_suffixed_counts_are_read() {
        assert_eq!(parse("4096"), Ok(4096));
        assert_eq!(parse("0x1000"), Ok(4096));
        assert_eq!(parse("64K"), Ok(64 << 10));
        assert_eq!(parse("2MiB"), Ok(2 << 20));
        assert_eq!(parse("1g"), Ok(1 << 30));
        assert_eq!(parse("3TB"), Ok(3 << 40));
    }

    #[test]
    fn nonsense_and_overflow_are_refused() {
        assert!(parse("").is_err());
        assert!(parse("K").is_err());
        assert!(parse("12Q").is_err());
        assert!(parse("-1").is_err());
        assert!(parse("99999999999T").is_err());
    }
}
