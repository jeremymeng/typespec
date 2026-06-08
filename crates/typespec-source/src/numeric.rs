use num_bigint::{BigInt, Sign};
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone)]
pub struct Numeric {
    /// Mantissa digits as BigInt (unsigned magnitude; sign is separate).
    pub(crate) n: BigInt,
    /// Exponent (base 10).
    pub(crate) e: i32,
    /// Number of decimal digits.
    pub(crate) d: u32,
    /// Sign: 1 or -1; never 0 (use n == 0 for zero).
    pub(crate) s: i8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidNumericError(pub String);

impl Numeric {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, InvalidNumericError> {
        parse(s)
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.as_bigint()?.try_into().ok()
    }

    pub fn as_u64(&self) -> Option<u64> {
        self.as_bigint()?.try_into().ok()
    }

    pub fn as_bigint(&self) -> Option<BigInt> {
        if !self.is_integer() {
            return None;
        }
        Some(if self.s == -1 {
            -&self.n
        } else {
            self.n.clone()
        })
    }

    /// Return the value as an f64. This is intentionally lossy and is only for diagnostics
    /// and backwards-compatible JS convenience fields.
    pub fn as_f64(&self) -> f64 {
        self.to_string().parse::<f64>().unwrap_or({
            if self.s == -1 {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }
        })
    }

    pub fn as_number(&self) -> Option<f64> {
        let value = self.as_f64();
        if !value.is_finite() {
            return None;
        }
        let roundtrip = Numeric::from_str(&js_like_number_to_string(value)).ok()?;
        if self.equals(&roundtrip) {
            Some(value)
        } else {
            None
        }
    }

    pub fn is_integer(&self) -> bool {
        self.d == 0
    }

    pub fn equals(&self, other: &Self) -> bool {
        self.n == other.n && self.e == other.e
    }

    pub fn gt(&self, other: &Self) -> bool {
        self.compare(other) == Ordering::Greater
    }

    pub fn lt(&self, other: &Self) -> bool {
        self.compare(other) == Ordering::Less
    }

    pub fn gte(&self, other: &Self) -> bool {
        self.compare(other) != Ordering::Less
    }

    pub fn lte(&self, other: &Self) -> bool {
        self.compare(other) != Ordering::Greater
    }

    fn compare(&self, other: &Self) -> Ordering {
        if self.s < other.s {
            return Ordering::Less;
        } else if self.s > other.s {
            return Ordering::Greater;
        }

        let sign = self.s;
        if self.e < other.e {
            return if sign == -1 {
                Ordering::Greater
            } else {
                Ordering::Less
            };
        } else if self.e > other.e {
            return if sign == -1 {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }

        let mut a = self.n.clone();
        let mut b = other.n.clone();
        if self.d < other.d {
            a *= pow10(other.d - self.d);
        } else if self.d > other.d {
            b *= pow10(self.d - other.d);
        }

        match a.cmp(&b) {
            Ordering::Less if sign == -1 => Ordering::Greater,
            Ordering::Greater if sign == -1 => Ordering::Less,
            other => other,
        }
    }
}

impl PartialEq for Numeric {
    fn eq(&self, other: &Self) -> bool {
        self.equals(other)
    }
}

impl Eq for Numeric {}

impl PartialOrd for Numeric {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Numeric {
    fn cmp(&self, other: &Self) -> Ordering {
        if self.equals(other) {
            Ordering::Equal
        } else {
            self.compare(other)
        }
    }
}

impl Hash for Numeric {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.n.hash(state);
        self.e.hash(state);
    }
}

impl fmt::Display for Numeric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.n == BigInt::from(0) {
            return f.write_str("0");
        }

        let n = self.n.to_string();
        let sign = if self.s == -1 { "-" } else { "" };
        let int = if self.e <= 0 {
            "0"
        } else {
            n.get(..(self.e as usize).min(n.len())).unwrap_or("")
        };
        let decimal = if self.e < n.len() as i32 {
            let d = self.d as usize;
            let start = n.len().saturating_sub(d);
            let suffix = &n[start..];
            format!(".{suffix:0>width$}", width = d)
        } else {
            String::new()
        };
        write!(f, "{sign}{int}{decimal}")
    }
}

impl std::str::FromStr for Numeric {
    type Err = InvalidNumericError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Numeric::from_str(s)
    }
}

impl fmt::Display for InvalidNumericError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidNumericError {}

fn parse(original: &str) -> Result<Numeric, InvalidNumericError> {
    let mut string_value = original.replace('_', "");
    let mut start = 0usize;
    let mut sign = 1i8;
    let mut decimal: Option<u32> = None;

    if string_value.as_bytes().first() == Some(&b'-') {
        start = 1;
        sign = -1;
    }

    let second = string_value
        .as_bytes()
        .get(start + 1)
        .copied()
        .map(|b| b.to_ascii_lowercase());

    let (n, exp) = if string_value.as_bytes().get(start) == Some(&b'0')
        && matches!(second, Some(b'b' | b'x' | b'o'))
    {
        let radix = match second.unwrap() {
            b'b' => 2,
            b'o' => 8,
            b'x' => 16,
            _ => unreachable!(),
        };
        let digits = &string_value[start + 2..];
        let Some(n) = BigInt::parse_bytes(digits.as_bytes(), radix) else {
            return Err(invalid(original));
        };
        decimal = Some(0);
        let exp = n.to_string().len() as i32;
        (n, exp)
    } else {
        while string_value.as_bytes().get(start) == Some(&b'0') {
            start += 1;
        }

        let decimal_point_index = string_value.find('.').map(|x| x as i32).unwrap_or(-1);
        let adjusted_point_index = decimal_point_index - start as i32;
        let mut exp = None;
        if decimal_point_index != -1 {
            exp = Some(adjusted_point_index);
            string_value.remove(decimal_point_index as usize);
        }

        if let Some(i) = find_exponent(&string_value).filter(|i| *i > 0) {
            if exp.is_none() {
                exp = Some(i as i32 - start as i32);
            }
            let exponent = string_value[i + 1..]
                .parse::<i32>()
                .map_err(|_| invalid(original))?;
            exp = Some(exp.unwrap() + exponent);
            string_value = string_value[start..i].to_string();
            decimal = Some((string_value.len() as i32 - exp.unwrap()).max(0) as u32);
        } else if exp.is_none() {
            exp = Some(string_value.len() as i32 - start as i32);
            string_value = string_value[start..].to_string();
        } else {
            string_value = string_value[start..].to_string();
        }
        let mut exp = exp.unwrap();

        let mut end = string_value.len();
        while end > 0
            && string_value.as_bytes()[end - 1] == b'0'
            && end as i32 > adjusted_point_index
        {
            end -= 1;
        }

        if start as i32 == adjusted_point_index + 1 {
            let mut cur = adjusted_point_index;
            while cur >= 0 && (cur as usize) < end && string_value.as_bytes()[cur as usize] == b'0'
            {
                cur += 1;
                exp -= 1;
            }
        }

        string_value.truncate(end);
        if exp > string_value.len() as i32 {
            string_value.push_str(&"0".repeat((exp - string_value.len() as i32) as usize));
        }

        let n = if string_value.is_empty() {
            BigInt::from(0)
        } else {
            BigInt::parse_bytes(string_value.as_bytes(), 10).ok_or_else(|| invalid(original))?
        };
        if n == BigInt::from(0) {
            decimal = Some(0);
        } else if decimal.is_none() {
            decimal = Some((string_value.len() as i32 - exp.max(0)).max(0) as u32);
        }
        (n, exp)
    };

    Ok(Numeric {
        n,
        e: exp,
        d: decimal.unwrap_or(0),
        s: sign,
    })
}

fn find_exponent(value: &str) -> Option<usize> {
    value
        .as_bytes()
        .iter()
        .position(|b| matches!(b, b'e' | b'E'))
}

fn invalid(original: &str) -> InvalidNumericError {
    InvalidNumericError(format!("Invalid numeric value: {original}"))
}

fn pow10(exp: u32) -> BigInt {
    BigInt::from(10u8).pow(exp)
}

fn js_like_number_to_string(value: f64) -> String {
    if value == 0.0 {
        "0".to_string()
    } else {
        value.to_string()
    }
}

pub fn bigint_to_napi_words(value: &BigInt) -> (bool, Vec<u64>) {
    let (sign, words) = value.to_u64_digits();
    let sign_bit = sign == Sign::Minus;
    if words.is_empty() {
        (false, vec![0])
    } else {
        (sign_bit, words)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;

    fn numeric(value: &str) -> Numeric {
        Numeric::from_str(value).unwrap()
    }

    fn hash(value: &Numeric) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn parses_decimal_and_exponent_values() {
        let cases = [
            ("0", "0"),
            ("-0", "0"),
            ("1", "1"),
            ("-1", "-1"),
            ("1.5", "1.5"),
            ("-1.5", "-1.5"),
            ("1e10", "10000000000"),
            ("-1.5e-3", "-0.0015"),
            ("0.1", "0.1"),
            ("9223372036854775807", "9223372036854775807"),
            ("18446744073709551615", "18446744073709551615"),
            ("1.234567890123456789012345", "1.234567890123456789012345"),
            ("0b10000000000000000000000000000000", "2147483648"),
            ("0o755", "493"),
            ("0xFFFFFFFFFFFFFFFFF", "295147905179352825855"),
        ];

        for (input, output) in cases {
            assert_eq!(numeric(input).to_string(), output, "{input}");
        }
    }

    #[test]
    fn rejects_invalid_values() {
        for input in ["0babc", "0xGHI", "0o999", "a123", "1d.3", "1.2.3"] {
            assert!(Numeric::from_str(input).is_err(), "{input}");
        }
    }

    #[test]
    fn equality_uses_canonical_numeric_value() {
        let pairs = [
            ("1.0", "1"),
            ("0", "-0"),
            ("1e2", "100"),
            ("123.4500", "123.45000"),
            ("00123", "123"),
        ];

        for (left, right) in pairs {
            assert_eq!(numeric(left), numeric(right), "{left} == {right}");
            assert_eq!(hash(&numeric(left)), hash(&numeric(right)));
        }
    }

    #[test]
    fn inequality_preserves_decimal_precision() {
        assert_ne!(numeric("0.30000000000000004"), numeric("0.3"));
        assert_ne!(
            numeric("1.234567890123456789012345"),
            numeric("1.234567890123456789012346")
        );
    }

    #[test]
    fn orders_values_exactly() {
        assert!(numeric("1").lt(&numeric("2")));
        assert!(numeric("1.5").gt(&numeric("1")));
        assert!(numeric("-1").lt(&numeric("0")));
        assert!(numeric("9223372036854775807").lt(&numeric("9223372036854775808")));
        assert!(numeric("123456789123456789.123456789123456789")
            .lt(&numeric("123456789123456789.1234567891234567891")));
    }

    #[test]
    fn converts_exact_integer_ranges() {
        assert_eq!(numeric("9223372036854775807").as_i64(), Some(i64::MAX));
        assert_eq!(numeric("-9223372036854775808").as_i64(), Some(i64::MIN));
        assert_eq!(numeric("9223372036854775808").as_i64(), None);
        assert_eq!(numeric("-9223372036854775809").as_i64(), None);
        assert_eq!(numeric("18446744073709551615").as_u64(), Some(u64::MAX));
        assert_eq!(numeric("18446744073709551616").as_u64(), None);
        assert_eq!(numeric("-1").as_u64(), None);
        assert_eq!(numeric("1.5").as_i64(), None);
    }

    #[test]
    fn as_number_returns_only_lossless_values() {
        assert_eq!(numeric("123").as_number(), Some(123.0));
        assert_eq!(numeric("0.1").as_number(), Some(0.1));
        assert_eq!(
            numeric("9007199254740991").as_number(),
            Some(9007199254740991.0)
        );
        assert_eq!(numeric("9007199254740993").as_number(), None);
        assert_eq!(
            numeric("123456789123456789.123456789123456789").as_number(),
            None
        );
    }
}
