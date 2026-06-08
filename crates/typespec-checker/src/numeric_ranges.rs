use crate::types::Type;
use typespec_source::Numeric;

#[derive(Debug, Clone, PartialEq)]
pub struct NumericRange {
    pub min: Numeric,
    pub max: Numeric,
    pub unbounded: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NumericScalarInfo {
    pub range: NumericRange,
    pub integer: bool,
}

pub fn scalar_range(intrinsic: &Type) -> Option<NumericRange> {
    numeric_scalar_info(intrinsic).map(|info| info.range)
}

pub fn numeric_scalar_info(intrinsic: &Type) -> Option<NumericScalarInfo> {
    let info = match intrinsic {
        Type::Int8 => NumericScalarInfo {
            range: range("-128", "127"),
            integer: true,
        },
        Type::Int16 => NumericScalarInfo {
            range: range("-32768", "32767"),
            integer: true,
        },
        Type::Int32 => NumericScalarInfo {
            range: range("-2147483648", "2147483647"),
            integer: true,
        },
        Type::Int64 => NumericScalarInfo {
            range: range("-9223372036854775808", "9223372036854775807"),
            integer: true,
        },
        Type::Uint8 => NumericScalarInfo {
            range: range("0", "255"),
            integer: true,
        },
        Type::Uint16 => NumericScalarInfo {
            range: range("0", "65535"),
            integer: true,
        },
        Type::Uint32 => NumericScalarInfo {
            range: range("0", "4294967295"),
            integer: true,
        },
        Type::Uint64 => NumericScalarInfo {
            range: range("0", "18446744073709551615"),
            integer: true,
        },
        Type::SafeInt => NumericScalarInfo {
            range: range("-9007199254740991", "9007199254740991"),
            integer: true,
        },
        Type::Float32 => NumericScalarInfo {
            range: range("-3.4e38", "3.4e38"),
            integer: false,
        },
        Type::Float64 => NumericScalarInfo {
            range: range("-1.7976931348623157e308", "1.7976931348623157e308"),
            integer: false,
        },
        Type::Integer => NumericScalarInfo {
            range: unbounded(),
            integer: true,
        },
        Type::Numeric | Type::Float | Type::Decimal | Type::Decimal128 => NumericScalarInfo {
            range: unbounded(),
            integer: false,
        },
        _ => return None,
    };
    Some(info)
}

pub fn is_in_range(value: &Numeric, range: &NumericRange) -> bool {
    range.unbounded || (value.gte(&range.min) && value.lte(&range.max))
}

pub fn ranges_overlap(a: &NumericRange, b: &NumericRange) -> bool {
    a.unbounded || b.unbounded || (a.min.lte(&b.max) && b.min.lte(&a.max))
}

pub fn range_subsumes(outer: &NumericRange, inner: &NumericRange) -> bool {
    outer.unbounded || (!inner.unbounded && outer.min.lte(&inner.min) && outer.max.gte(&inner.max))
}

fn range(min: &str, max: &str) -> NumericRange {
    NumericRange {
        min: Numeric::from_str(min).unwrap(),
        max: Numeric::from_str(max).unwrap(),
        unbounded: false,
    }
}

fn unbounded() -> NumericRange {
    NumericRange {
        min: Numeric::from_str("0").unwrap(),
        max: Numeric::from_str("0").unwrap(),
        unbounded: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_ranges() {
        let int8 = scalar_range(&Type::Int8).unwrap();
        let int32 = scalar_range(&Type::Int32).unwrap();
        assert!(is_in_range(&Numeric::from_str("42").unwrap(), &int8));
        assert!(!is_in_range(&Numeric::from_str("9999").unwrap(), &int8));
        assert!(range_subsumes(&int32, &int8));
        assert!(ranges_overlap(&int8, &int32));
        assert!(is_in_range(
            &Numeric::from_str("9223372036854775807").unwrap(),
            &scalar_range(&Type::Int64).unwrap()
        ));
        assert!(!is_in_range(
            &Numeric::from_str("9223372036854775808").unwrap(),
            &scalar_range(&Type::Int64).unwrap()
        ));
    }
}
