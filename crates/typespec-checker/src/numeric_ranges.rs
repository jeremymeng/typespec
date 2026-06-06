use crate::types::Type;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumericRange {
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumericScalarInfo {
    pub range: NumericRange,
    pub integer: bool,
}

pub fn scalar_range(intrinsic: &Type) -> Option<NumericRange> {
    numeric_scalar_info(intrinsic).map(|info| info.range)
}

pub fn numeric_scalar_info(intrinsic: &Type) -> Option<NumericScalarInfo> {
    let any = NumericRange {
        min: f64::NEG_INFINITY,
        max: f64::INFINITY,
    };
    let info = match intrinsic {
        Type::Int8 => NumericScalarInfo {
            range: NumericRange {
                min: -128.0,
                max: 127.0,
            },
            integer: true,
        },
        Type::Int16 => NumericScalarInfo {
            range: NumericRange {
                min: -32768.0,
                max: 32767.0,
            },
            integer: true,
        },
        Type::Int32 => NumericScalarInfo {
            range: NumericRange {
                min: -2147483648.0,
                max: 2147483647.0,
            },
            integer: true,
        },
        Type::Int64 => NumericScalarInfo {
            range: NumericRange {
                min: -9223372036854775808.0,
                max: 9223372036854775807.0,
            },
            integer: true,
        },
        Type::Uint8 => NumericScalarInfo {
            range: NumericRange {
                min: 0.0,
                max: 255.0,
            },
            integer: true,
        },
        Type::Uint16 => NumericScalarInfo {
            range: NumericRange {
                min: 0.0,
                max: 65535.0,
            },
            integer: true,
        },
        Type::Uint32 => NumericScalarInfo {
            range: NumericRange {
                min: 0.0,
                max: 4294967295.0,
            },
            integer: true,
        },
        Type::Uint64 => NumericScalarInfo {
            range: NumericRange {
                min: 0.0,
                max: 18446744073709551615.0,
            },
            integer: true,
        },
        Type::SafeInt => NumericScalarInfo {
            range: NumericRange {
                min: -9007199254740991.0,
                max: 9007199254740991.0,
            },
            integer: true,
        },
        Type::Float32 => NumericScalarInfo {
            range: NumericRange {
                min: -3.4e38,
                max: 3.4e38,
            },
            integer: false,
        },
        Type::Float64 => NumericScalarInfo {
            range: NumericRange {
                min: -f64::MAX,
                max: f64::MAX,
            },
            integer: false,
        },
        Type::Integer => NumericScalarInfo {
            range: any,
            integer: true,
        },
        Type::Numeric | Type::Float | Type::Decimal | Type::Decimal128 => NumericScalarInfo {
            range: any,
            integer: false,
        },
        _ => return None,
    };
    Some(info)
}

pub fn is_in_range(value: f64, range: &NumericRange) -> bool {
    value >= range.min && value <= range.max
}

pub fn ranges_overlap(a: &NumericRange, b: &NumericRange) -> bool {
    a.min <= b.max && b.min <= a.max
}

pub fn range_subsumes(outer: &NumericRange, inner: &NumericRange) -> bool {
    outer.min <= inner.min && outer.max >= inner.max
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_ranges() {
        let int8 = scalar_range(&Type::Int8).unwrap();
        let int32 = scalar_range(&Type::Int32).unwrap();
        assert!(is_in_range(42.0, &int8));
        assert!(!is_in_range(9999.0, &int8));
        assert!(range_subsumes(&int32, &int8));
        assert!(ranges_overlap(&int8, &int32));
    }
}
