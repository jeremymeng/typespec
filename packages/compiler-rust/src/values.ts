// Materialize values serialized by the Rust checker (decorator arguments, property defaults)
// as TS-shaped `Value`s on the Program adapter.

import { Numeric } from "@typespec/compiler";
import type { CompositeValue, EnumMember, NodeShim, Program, Type } from "./program.js";

export interface SerializedValue {
  valueKind: string;
  value?: string | boolean;
  typeId?: number;
  properties?: Array<[string, SerializedValue]>;
  values?: SerializedValue[];
}

export interface ValueShim {
  entityKind: "Value";
  valueKind: string;
  value: unknown;
  type: Type;
  scalar: undefined;
  node?: NodeShim;
}

export function createValue(
  program: Program,
  serialized: SerializedValue,
  node: NodeShim | undefined,
): ValueShim | undefined {
  const literalType = (fallback: () => Type) =>
    serialized.typeId === undefined
      ? fallback()
      : (program._typeCache.get(serialized.typeId) ?? fallback());
  const value = (valueKind: string, raw: unknown, type: Type): ValueShim => ({
    entityKind: "Value",
    valueKind,
    value: raw,
    type,
    scalar: undefined,
    ...(node ? { node } : {}),
  });
  switch (serialized.valueKind) {
    case "StringValue": {
      const raw = serialized.value as string;
      return value(
        "StringValue",
        raw,
        literalType(() => program.checker.createLiteralType(raw)),
      );
    }
    case "NumericValue": {
      const raw = Numeric(serialized.value as string);
      return value(
        "NumericValue",
        raw,
        literalType(() => program.checker.createLiteralType(raw)),
      );
    }
    case "BooleanValue": {
      const raw = serialized.value as boolean;
      return value(
        "BooleanValue",
        raw,
        literalType(() => program.checker.createLiteralType(raw)),
      );
    }
    case "NullValue":
      return value("NullValue", null, program.checker.nullType);
    case "EnumValue": {
      const member = program._typeCache.get(serialized.typeId!) as EnumMember | undefined;
      if (!member) return undefined;
      return value("EnumValue", member, member.enum);
    }
    case "ObjectValue": {
      const properties = new Map<string, { name: string; value: ValueShim }>();
      for (const [name, property] of serialized.properties ?? []) {
        const propertyValue = createValue(program, property, undefined);
        if (!propertyValue) return undefined;
        properties.set(name, { name, value: propertyValue });
      }
      const objectValue = {
        entityKind: "Value",
        valueKind: "ObjectValue",
        properties,
        scalar: undefined,
        ...(node ? { node } : {}),
      } as unknown as ValueShim & CompositeValue;
      objectValue.type = program._createCompositeValueType(objectValue);
      return objectValue;
    }
    case "ArrayValue": {
      const values: ValueShim[] = [];
      for (const item of serialized.values ?? []) {
        const itemValue = createValue(program, item, undefined);
        if (!itemValue) return undefined;
        values.push(itemValue);
      }
      const arrayValue = {
        entityKind: "Value",
        valueKind: "ArrayValue",
        values,
        scalar: undefined,
        ...(node ? { node } : {}),
      } as unknown as ValueShim & CompositeValue;
      arrayValue.type = program._createCompositeValueType(arrayValue);
      return arrayValue;
    }
    default:
      // Const references, scalar constructors, ... (M12 value system).
      return undefined;
  }
}
