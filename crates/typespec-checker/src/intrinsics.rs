use std::collections::HashMap;

use crate::types::{SymbolId, SymbolTable, Type, TypeArena, TypeId};

#[derive(Debug, Clone)]
pub struct Intrinsics {
    pub string: TypeId,
    pub bytes: TypeId,
    pub int8: TypeId,
    pub int16: TypeId,
    pub int32: TypeId,
    pub int64: TypeId,
    pub uint8: TypeId,
    pub uint16: TypeId,
    pub uint32: TypeId,
    pub uint64: TypeId,
    pub float: TypeId,
    pub float32: TypeId,
    pub float64: TypeId,
    pub decimal: TypeId,
    pub decimal128: TypeId,
    pub integer: TypeId,
    pub numeric: TypeId,
    pub safeint: TypeId,
    pub boolean: TypeId,
    pub url: TypeId,
    pub plain_date: TypeId,
    pub plain_time: TypeId,
    pub utc_date_time: TypeId,
    pub offset_date_time: TypeId,
    pub duration: TypeId,
    pub void: TypeId,
    pub never: TypeId,
    pub unknown: TypeId,
    pub null: TypeId,
}

impl Intrinsics {
    pub fn by_name(&self, name: &str) -> Option<TypeId> {
        match name {
            "string" => Some(self.string),
            "bytes" => Some(self.bytes),
            "int8" => Some(self.int8),
            "int16" => Some(self.int16),
            "int32" => Some(self.int32),
            "int64" => Some(self.int64),
            "uint8" => Some(self.uint8),
            "uint16" => Some(self.uint16),
            "uint32" => Some(self.uint32),
            "uint64" => Some(self.uint64),
            "float" => Some(self.float),
            "float32" => Some(self.float32),
            "float64" => Some(self.float64),
            "decimal" => Some(self.decimal),
            "decimal128" => Some(self.decimal128),
            "integer" => Some(self.integer),
            "numeric" => Some(self.numeric),
            "safeint" => Some(self.safeint),
            "boolean" => Some(self.boolean),
            "url" => Some(self.url),
            "plainDate" => Some(self.plain_date),
            "plainTime" => Some(self.plain_time),
            "utcDateTime" => Some(self.utc_date_time),
            "offsetDateTime" => Some(self.offset_date_time),
            "duration" => Some(self.duration),
            "void" => Some(self.void),
            "never" => Some(self.never),
            "unknown" => Some(self.unknown),
            "null" => Some(self.null),
            _ => None,
        }
    }
}

pub fn register_intrinsics(
    arena: &mut TypeArena,
    symbol_table: &SymbolTable,
    symbol_to_type: &mut HashMap<SymbolId, TypeId>,
) -> Intrinsics {
    let numeric = arena.add(Type::Numeric);
    let integer = arena.add(Type::Integer);
    let int64 = arena.add(Type::Int64);
    let int32 = arena.add(Type::Int32);
    let int16 = arena.add(Type::Int16);
    let int8 = arena.add(Type::Int8);
    let uint64 = arena.add(Type::Uint64);
    let uint32 = arena.add(Type::Uint32);
    let uint16 = arena.add(Type::Uint16);
    let uint8 = arena.add(Type::Uint8);
    let safeint = arena.add(Type::SafeInt);
    let float = arena.add(Type::Float);
    let float64 = arena.add(Type::Float64);
    let float32 = arena.add(Type::Float32);
    let decimal = arena.add(Type::Decimal);
    let decimal128 = arena.add(Type::Decimal128);
    let string = arena.add(Type::String);
    let bytes = arena.add(Type::Bytes);
    let boolean = arena.add(Type::Boolean);
    let plain_date = arena.add(Type::PlainDate);
    let plain_time = arena.add(Type::PlainTime);
    let utc_date_time = arena.add(Type::UtcDateTime);
    let offset_date_time = arena.add(Type::OffsetDateTime);
    let duration = arena.add(Type::Duration);
    let url = arena.add(Type::Url);
    let void = arena.add(Type::Void);
    let never = arena.add(Type::Never);
    let unknown = arena.add(Type::Unknown);
    let null = arena.add(Type::Null);

    let intrinsics = Intrinsics {
        string,
        bytes,
        int8,
        int16,
        int32,
        int64,
        uint8,
        uint16,
        uint32,
        uint64,
        float,
        float32,
        float64,
        decimal,
        decimal128,
        integer,
        numeric,
        safeint,
        boolean,
        url,
        plain_date,
        plain_time,
        utc_date_time,
        offset_date_time,
        duration,
        void,
        never,
        unknown,
        null,
    };

    for (name, type_id) in [
        ("numeric", numeric),
        ("integer", integer),
        ("int64", int64),
        ("int32", int32),
        ("int16", int16),
        ("int8", int8),
        ("uint64", uint64),
        ("uint32", uint32),
        ("uint16", uint16),
        ("uint8", uint8),
        ("safeint", safeint),
        ("float", float),
        ("float64", float64),
        ("float32", float32),
        ("decimal", decimal),
        ("decimal128", decimal128),
        ("string", string),
        ("bytes", bytes),
        ("boolean", boolean),
        ("plainDate", plain_date),
        ("plainTime", plain_time),
        ("utcDateTime", utc_date_time),
        ("offsetDateTime", offset_date_time),
        ("duration", duration),
        ("url", url),
        ("void", void),
        ("never", never),
        ("unknown", unknown),
        ("null", null),
    ] {
        if let Some(symbol) = symbol_table.scopes[symbol_table.global_scope.index()]
            .symbols
            .get(name)
            .copied()
        {
            symbol_to_type.insert(symbol, type_id);
        }
    }

    intrinsics
}
