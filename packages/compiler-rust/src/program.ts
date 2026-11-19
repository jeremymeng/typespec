import type { CheckResultJs, DecoratorStateEntry, SerializedType } from "../index.js";
import {
  createDiagnosticCollector,
  type Diagnostic,
  type SerializedDiagnostic,
} from "./diagnostics.js";

export interface Program {
  checker: Checker;
  diagnostics: readonly Diagnostic[];
  reportDiagnostic(diagnostic: Diagnostic): void;
  reportDiagnostics(diagnostics: readonly Diagnostic[]): void;
  hasError(): boolean;
  getGlobalNamespaceType(): Namespace;
  _decoratorState: WeakMap<Type, Record<string, unknown>>;
  _typeCache: ReadonlyMap<number, Type>;
}

export interface Checker {
  getGlobalNamespaceType(): Namespace;
}

export interface DecoratorApplication {
  decorator: string;
  args: unknown[];
}

interface BaseType {
  readonly id: number;
  readonly entityKind: "Type";
  readonly serializedKind: string;
  kind: string;
  decorators: DecoratorApplication[];
  isFinished: boolean;
}

export interface Namespace extends BaseType {
  kind: "Namespace";
  name: string;
  namespace?: Namespace;
  models: Map<string, Model>;
  scalars: Map<string, Scalar>;
  enums: Map<string, Enum>;
  unions: Map<string, Union>;
  aliases: Map<string, Alias>;
  operations: Map<string, Operation>;
  interfaces: Map<string, Interface>;
  namespaces: Map<string, Namespace>;
  decoratorDeclarations: Map<string, never>;
  functionDeclarations: Map<string, never>;
}

export interface Model extends BaseType {
  kind: "Model";
  name: string;
  namespace?: Namespace;
  properties: Map<string, ModelProperty>;
  baseModel?: Model;
  sourceModel?: Model;
  sourceModels: Array<{ usage: "is"; model: Model }>;
  derivedModels: Model[];
  indexer?: { key: Scalar; value: Type };
  expression: boolean;
  templateArguments?: Type[];
  templateParameters?: TemplateParameter[];
  isTemplateDeclaration: boolean;
}

export interface ModelProperty extends BaseType {
  kind: "ModelProperty";
  name: string;
  type: Type;
  sourceProperty?: ModelProperty;
  optional: boolean;
  defaultValue?: Type;
  model?: Model;
}

export interface Scalar extends BaseType {
  kind: "Scalar";
  name: string;
  namespace?: Namespace;
  baseScalar?: Scalar;
  derivedScalars: Scalar[];
  constructors: Map<string, never>;
  expression: boolean;
  templateArguments?: Type[];
  templateParameters?: TemplateParameter[];
  isTemplateDeclaration: boolean;
}

export interface Enum extends BaseType {
  kind: "Enum";
  name: string;
  namespace?: Namespace;
  members: Map<string, EnumMember>;
  expression: boolean;
}

export interface EnumMember extends BaseType {
  kind: "EnumMember";
  name: string;
  enum: Enum;
  value?: string | number | bigint;
}

export interface Union extends BaseType {
  kind: "Union";
  name: string;
  namespace?: Namespace;
  variants: Map<string, UnionVariant>;
  expression: boolean;
  baseType?: Model | Scalar | Enum | Union;
  templateArguments?: Type[];
  templateParameters?: TemplateParameter[];
  isTemplateDeclaration: boolean;
}

export interface UnionVariant extends BaseType {
  kind: "UnionVariant";
  name: string;
  type: Type;
  union: Union;
}

export interface Operation extends BaseType {
  kind: "Operation";
  name: string;
  namespace?: Namespace;
  interface?: Interface;
  parameters: Model;
  returnType: Type;
  sourceOperation?: Operation;
  templateArguments?: Type[];
  templateParameters?: TemplateParameter[];
  isTemplateDeclaration: boolean;
}

export interface Interface extends BaseType {
  kind: "Interface";
  name: string;
  namespace?: Namespace;
  sourceInterfaces: Interface[];
  operations: Map<string, Operation>;
  templateArguments?: Type[];
  templateParameters?: TemplateParameter[];
  isTemplateDeclaration: boolean;
}

export interface Alias extends BaseType {
  kind: "Alias";
  name: string;
  namespace?: Namespace;
  target: Type;
  templateArguments?: Type[];
  templateParameters?: TemplateParameter[];
  isTemplateDeclaration: boolean;
}

export interface StringLiteral extends BaseType {
  kind: "String";
  value: string;
}

export interface NumericLiteral extends BaseType {
  kind: "Number";
  value: number;
  numericValue: { toString(): string };
  valueAsString: string;
}

export interface BooleanLiteral extends BaseType {
  kind: "Boolean";
  value: boolean;
}

export interface Intrinsic extends BaseType {
  kind: "Intrinsic";
  name: "ErrorType" | "void" | "never" | "unknown" | "null";
}

export interface TemplateParameter extends BaseType {
  kind: "TemplateParameter";
  name: string;
  constraint?: Type;
  default?: Type;
}

export type Type =
  | Namespace
  | Model
  | ModelProperty
  | Scalar
  | Enum
  | EnumMember
  | Union
  | UnionVariant
  | Operation
  | Interface
  | Alias
  | StringLiteral
  | NumericLiteral
  | BooleanLiteral
  | Intrinsic
  | TemplateParameter;

const intrinsicScalars = new Map<string, string>([
  ["String", "string"],
  ["Bytes", "bytes"],
  ["Int8", "int8"],
  ["Int16", "int16"],
  ["Int32", "int32"],
  ["Int64", "int64"],
  ["Uint8", "uint8"],
  ["Uint16", "uint16"],
  ["Uint32", "uint32"],
  ["Uint64", "uint64"],
  ["Float", "float"],
  ["Float32", "float32"],
  ["Float64", "float64"],
  ["Decimal", "decimal"],
  ["Decimal128", "decimal128"],
  ["Integer", "integer"],
  ["Numeric", "numeric"],
  ["SafeInt", "safeint"],
  ["Boolean", "boolean"],
  ["Url", "url"],
  ["PlainDate", "plainDate"],
  ["PlainTime", "plainTime"],
  ["UtcDateTime", "utcDateTime"],
  ["OffsetDateTime", "offsetDateTime"],
  ["Duration", "duration"],
]);

const intrinsicTypes = new Map<string, Intrinsic["name"]>([
  ["Void", "void"],
  ["Never", "never"],
  ["Unknown", "unknown"],
  ["Null", "null"],
  ["Error", "ErrorType"],
]);

export function createProgram(checkResult: CheckResultJs): Program {
  const serializedById = new Map(checkResult.types.map((type) => [type.id, type]));
  const typeCache = new Map<number, Type>();
  const globalNamespaceId = findGlobalNamespaceId(checkResult.types);
  const decoratorState = new WeakMap<Type, Record<string, unknown>>();
  const diagnosticCollector = createDiagnosticCollector(
    checkResult.diagnostics as readonly SerializedDiagnostic[],
  );

  const checker: Checker = {
    getGlobalNamespaceType: () => getOrCreate(globalNamespaceId) as Namespace,
  };

  const program: Program = {
    checker,
    diagnostics: diagnosticCollector.diagnostics,
    reportDiagnostic: diagnosticCollector.reportDiagnostic,
    reportDiagnostics(diagnostics: readonly Diagnostic[]) {
      for (const diagnostic of diagnostics) {
        diagnosticCollector.reportDiagnostic(diagnostic);
      }
    },
    hasError() {
      return diagnosticCollector.diagnostics.some((diagnostic) => diagnostic.severity === "error");
    },
    getGlobalNamespaceType: checker.getGlobalNamespaceType,
    _decoratorState: decoratorState,
    _typeCache: typeCache,
  };

  getOrCreate(globalNamespaceId);
  for (const type of checkResult.types) {
    getOrCreate(type.id);
  }
  populateReverseLinks();
  populateDecoratorState(checkResult.decoratorState);
  return program;

  function getOrCreate(typeId: number): Type {
    const cached = typeCache.get(typeId);
    if (cached) {
      return cached;
    }

    const serialized = serializedById.get(typeId);
    if (!serialized) {
      throw new Error(`Missing serialized type ${typeId}`);
    }

    const shell = createShell(serialized);
    typeCache.set(typeId, shell);
    populateType(shell, serialized);
    return shell;
  }

  function createShell(serialized: SerializedType): Type {
    const base = {
      id: serialized.id,
      entityKind: "Type" as const,
      serializedKind: serialized.kind,
      decorators: createDecoratorApplications(serialized),
      isFinished: !serialized.isTemplateDeclaration,
    };

    const scalarName = intrinsicScalars.get(serialized.kind);
    if (scalarName) {
      return {
        ...base,
        kind: "Scalar",
        name: scalarName,
        derivedScalars: [],
        constructors: new Map<string, never>(),
        expression: false,
        isTemplateDeclaration: false,
      };
    }

    const intrinsicName = intrinsicTypes.get(serialized.kind);
    if (intrinsicName) {
      return { ...base, kind: "Intrinsic", name: intrinsicName };
    }

    switch (serialized.kind) {
      case "Namespace":
        return {
          ...base,
          kind: "Namespace",
          name: serialized.name,
          models: new Map(),
          scalars: new Map(),
          enums: new Map(),
          unions: new Map(),
          aliases: new Map(),
          operations: new Map(),
          interfaces: new Map(),
          namespaces: new Map(),
          decoratorDeclarations: new Map<string, never>(),
          functionDeclarations: new Map<string, never>(),
        };
      case "Model":
        return {
          ...base,
          kind: "Model",
          name: serialized.name,
          properties: new Map(),
          sourceModels: [],
          derivedModels: [],
          expression: serialized.expression ?? false,
          isTemplateDeclaration: serialized.isTemplateDeclaration,
        };
      case "ModelProperty":
        return {
          ...base,
          kind: "ModelProperty",
          name: serialized.name,
          type: undefinedTypePlaceholder(),
          optional: serialized.optional ?? false,
        };
      case "Scalar":
        return {
          ...base,
          kind: "Scalar",
          name: serialized.name,
          derivedScalars: [],
          constructors: new Map<string, never>(),
          expression: serialized.expression ?? false,
          isTemplateDeclaration: serialized.isTemplateDeclaration,
        };
      case "Enum":
        return {
          ...base,
          kind: "Enum",
          name: serialized.name,
          members: new Map(),
          expression: serialized.expression ?? false,
        };
      case "EnumMember":
        return {
          ...base,
          kind: "EnumMember",
          name: serialized.name,
          enum: undefinedTypePlaceholder() as Enum,
        };
      case "Union":
        return {
          ...base,
          kind: "Union",
          name: serialized.name,
          variants: new Map(),
          expression: serialized.expression ?? false,
          isTemplateDeclaration: serialized.isTemplateDeclaration,
        };
      case "UnionVariant":
        return {
          ...base,
          kind: "UnionVariant",
          name: serialized.name,
          type: undefinedTypePlaceholder(),
          union: undefinedTypePlaceholder() as Union,
        };
      case "Operation":
        return {
          ...base,
          kind: "Operation",
          name: serialized.name,
          parameters: undefinedTypePlaceholder() as Model,
          returnType: undefinedTypePlaceholder(),
          isTemplateDeclaration: serialized.isTemplateDeclaration,
        };
      case "Interface":
        return {
          ...base,
          kind: "Interface",
          name: serialized.name,
          sourceInterfaces: [],
          operations: new Map(),
          isTemplateDeclaration: serialized.isTemplateDeclaration,
        };
      case "Alias":
        return {
          ...base,
          kind: "Alias",
          name: serialized.name,
          target: undefinedTypePlaceholder(),
          isTemplateDeclaration: serialized.isTemplateDeclaration,
        };
      case "StringLiteral":
        return { ...base, kind: "String", value: serialized.literalString ?? serialized.name };
      case "NumericLiteral": {
        const valueAsString = numericToString(serialized);
        return {
          ...base,
          kind: "Number",
          value: serialized.literalNumericNumber ?? Number(valueAsString),
          numericValue: { toString: () => valueAsString },
          valueAsString,
        };
      }
      case "BooleanLiteral":
        return { ...base, kind: "Boolean", value: serialized.literalBoolean ?? false };
      case "TemplateParameter":
        return { ...base, kind: "TemplateParameter", name: serialized.name };
      default:
        throw new Error(`Unsupported serialized type kind '${serialized.kind}'`);
    }
  }

  function populateType(type: Type, serialized: SerializedType): void {
    switch (type.kind) {
      case "Namespace":
        setOptionalNamespace(type, serialized.namespaceId);
        type.models = namedMap<Model>(serialized.modelNames, serialized.modelIds);
        type.scalars = namedMap<Scalar>(serialized.scalarNames, serialized.scalarIds);
        type.enums = namedMap<Enum>(serialized.enumNames, serialized.enumIds);
        type.unions = namedMap<Union>(serialized.unionNames, serialized.unionIds);
        type.aliases = namedMap<Alias>(serialized.aliasNames, serialized.aliasIds);
        type.operations = namedMap<Operation>(serialized.operationNames, serialized.operationIds);
        type.interfaces = namedMap<Interface>(serialized.interfaceNames, serialized.interfaceIds);
        type.namespaces = namedMap<Namespace>(serialized.namespaceNames, serialized.namespaceIds);
        break;
      case "Model":
        setOptionalNamespace(type, serialized.namespaceId);
        type.properties = namedMap<ModelProperty>(serialized.propertyNames, serialized.propertyIds);
        type.baseModel = optionalType<Model>(serialized.baseTypeId);
        type.sourceModel = optionalType<Model>(serialized.sourceModelId);
        if (type.sourceModel) {
          type.sourceModels = [{ usage: "is", model: type.sourceModel }];
        }
        type.templateArguments = typeArray(serialized.templateArgumentIds);
        type.templateParameters = typeArray<TemplateParameter>(serialized.templateParameterIds);
        if (serialized.indexerTypeId !== undefined && serialized.indexerKeyTypeId !== undefined) {
          type.indexer = {
            key: requiredType<Scalar>(serialized.indexerKeyTypeId, "model indexer key"),
            value: requiredType(serialized.indexerTypeId, "model indexer value"),
          };
        }
        break;
      case "ModelProperty":
        type.model = optionalType<Model>(serialized.baseTypeId);
        type.type = dereferenceAlias(
          requiredType(serialized.propertyTypeId, "model property type"),
        );
        type.sourceProperty = optionalType<ModelProperty>(serialized.sourcePropertyId);
        type.defaultValue = optionalType(serialized.defaultTypeId);
        break;
      case "Scalar":
        setOptionalNamespace(type, serialized.namespaceId);
        type.baseScalar = optionalType<Scalar>(serialized.baseTypeId);
        type.templateArguments = typeArray(serialized.templateArgumentIds);
        type.templateParameters = typeArray<TemplateParameter>(serialized.templateParameterIds);
        break;
      case "Enum":
        setOptionalNamespace(type, serialized.namespaceId);
        type.members = namedMap<EnumMember>(serialized.memberNames, serialized.memberIds);
        break;
      case "EnumMember":
        type.enum = requiredType<Enum>(serialized.baseTypeId, "enum member parent");
        type.value = enumMemberValue(serialized);
        break;
      case "Union":
        setOptionalNamespace(type, serialized.namespaceId);
        type.variants = namedMap<UnionVariant>(serialized.memberNames, serialized.memberIds);
        type.baseType = optionalType<Model | Scalar | Enum | Union>(serialized.baseTypeId);
        type.templateArguments = typeArray(serialized.templateArgumentIds);
        type.templateParameters = typeArray<TemplateParameter>(serialized.templateParameterIds);
        break;
      case "UnionVariant":
        type.union = requiredType<Union>(serialized.baseTypeId, "union variant parent");
        type.type = dereferenceAlias(requiredType(serialized.variantTypeId, "union variant type"));
        break;
      case "Operation":
        setOptionalNamespace(type, serialized.namespaceId);
        type.interface = optionalType<Interface>(serialized.interfaceId);
        type.parameters = requiredType<Model>(serialized.parametersTypeId, "operation parameters");
        type.returnType = dereferenceAlias(
          requiredType(serialized.returnTypeId, "operation return type"),
        );
        type.sourceOperation = optionalType<Operation>(serialized.sourceOperationId);
        type.templateArguments = typeArray(serialized.templateArgumentIds);
        type.templateParameters = typeArray<TemplateParameter>(serialized.templateParameterIds);
        break;
      case "Interface":
        setOptionalNamespace(type, serialized.namespaceId);
        type.operations = namedMap<Operation>(serialized.operationNames, serialized.operationIds);
        type.sourceInterfaces = typeArray<Interface>(serialized.sourceInterfaceIds);
        type.templateArguments = typeArray(serialized.templateArgumentIds);
        type.templateParameters = typeArray<TemplateParameter>(serialized.templateParameterIds);
        break;
      case "Alias":
        setOptionalNamespace(type, serialized.namespaceId);
        type.target = requiredType(serialized.targetId, "alias target");
        type.templateArguments = typeArray(serialized.templateArgumentIds);
        type.templateParameters = typeArray<TemplateParameter>(serialized.templateParameterIds);
        break;
      case "TemplateParameter":
        type.constraint = optionalType(serialized.constraintId);
        type.default = optionalType(serialized.templateDefaultId);
        break;
      case "String":
      case "Number":
      case "Boolean":
      case "Intrinsic":
        break;
    }
  }

  function populateReverseLinks(): void {
    for (const type of typeCache.values()) {
      if (type.kind === "Model" && type.baseModel) {
        type.baseModel.derivedModels.push(type);
      } else if (type.kind === "Scalar" && type.baseScalar) {
        type.baseScalar.derivedScalars.push(type);
      }
    }
  }

  function populateDecoratorState(entries: readonly DecoratorStateEntry[]): void {
    for (const entry of entries) {
      const target = getOrCreate(entry.typeId);
      const state = decoratorState.get(target) ?? {};
      state[entry.key] = decoratorStateValue(entry);
      decoratorState.set(target, state);
    }
  }

  function namedMap<T extends Type>(
    names: readonly string[],
    ids: readonly number[],
  ): Map<string, T> {
    const map = new Map<string, T>();
    const entries = names
      .map((name, index) => ({ name, id: ids[index] }))
      .filter((entry): entry is { name: string; id: number } => entry.id !== undefined)
      .sort((a, b) => declarationSortKey(a.id) - declarationSortKey(b.id));
    for (const { name, id } of entries) {
      map.set(name, getOrCreate(id) as T);
    }
    return map;
  }

  function typeArray<T extends Type = Type>(ids: readonly number[]): T[] {
    return ids.map((id) => getOrCreate(id) as T);
  }

  function optionalType<T extends Type = Type>(id: number | undefined): T | undefined {
    return id === undefined || id === null ? undefined : (getOrCreate(id) as T);
  }

  function requiredType<T extends Type = Type>(id: number | undefined, description: string): T {
    if (id === undefined || id === null) {
      throw new Error(`Missing ${description}`);
    }
    return getOrCreate(id) as T;
  }

  function dereferenceAlias(type: Type): Type {
    let current = type;
    const seen = new Set<Type>();
    while (current.kind === "Alias" && !seen.has(current)) {
      seen.add(current);
      current = current.target;
    }
    return current;
  }

  function declarationSortKey(id: number): number {
    return serializedById.get(id)?.nodeId ?? Number.MAX_SAFE_INTEGER + id;
  }

  function setOptionalNamespace(
    type: Namespace | Model | Scalar | Enum | Union | Operation | Interface | Alias,
    namespaceId: number | undefined,
  ): void {
    type.namespace = optionalType<Namespace>(namespaceId);
  }
}

function findGlobalNamespaceId(types: readonly SerializedType[]): number {
  const global = types.find(
    (type) => type.kind === "Namespace" && type.name === "" && type.namespaceId === undefined,
  );
  if (!global) {
    throw new Error("Rust check result did not contain a global namespace");
  }
  return global.id;
}

function createDecoratorApplications(type: SerializedType): DecoratorApplication[] {
  return type.decoratorNodeIds.map((id) => ({ decorator: `#${id}`, args: [] }));
}

function numericToString(type: SerializedType): string {
  if (type.literalNumericBigint !== undefined && type.literalNumericBigint !== null) {
    return type.literalNumericBigint.toString();
  }
  if (type.literalNumericString !== undefined && type.literalNumericString !== null) {
    return type.literalNumericString;
  }
  if (type.literalNumericNumber !== undefined && type.literalNumericNumber !== null) {
    return String(type.literalNumericNumber);
  }
  return type.name;
}

function enumMemberValue(type: SerializedType): string | number | bigint | undefined {
  if (type.enumMemberValueString !== undefined && type.enumMemberValueString !== null) {
    return type.enumMemberValueString;
  }
  if (
    type.enumMemberValueNumericNumber !== undefined &&
    type.enumMemberValueNumericNumber !== null
  ) {
    return type.enumMemberValueNumericNumber;
  }
  if (type.enumMemberValueNumeric !== undefined && type.enumMemberValueNumeric !== null) {
    return type.enumMemberValueNumeric;
  }
  if (
    type.enumMemberValueNumericBigint !== undefined &&
    type.enumMemberValueNumericBigint !== null
  ) {
    return type.enumMemberValueNumericBigint;
  }
  if (
    type.enumMemberValueNumericString !== undefined &&
    type.enumMemberValueNumericString !== null
  ) {
    return Number(type.enumMemberValueNumericString);
  }
  return undefined;
}

function decoratorStateValue(entry: DecoratorStateEntry): unknown {
  if (entry.valueString !== undefined) return entry.valueString;
  if (entry.valueNumber !== undefined) return entry.valueNumber;
  if (entry.valueBoolean !== undefined) return entry.valueBoolean;
  if (entry.valueTypeId !== undefined) return entry.valueTypeId;
  return null;
}

function undefinedTypePlaceholder(): Type {
  return undefined as unknown as Type;
}
