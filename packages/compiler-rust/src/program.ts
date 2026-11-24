import { getTypeName as compilerGetTypeName, type TypeNameOptions } from "@typespec/compiler";
import { SyntaxKind as compilerSyntaxKind } from "@typespec/compiler/ast";
import type {
  CheckProgramResultJs,
  CheckResultJs,
  DecoratorStateEntry,
  NodeInfoJs,
  SerializedType,
} from "../index.js";
import { isTypeAssignableTo as isTypeAssignableToNative } from "../index.js";
import {
  createDiagnosticCollector,
  type Diagnostic,
  type DiagnosticTarget,
  type SerializedDiagnostic,
} from "./diagnostics.js";
import type { EmitterHost } from "./host-fs.js";
import { createValue, type ValueShim } from "./values.js";

/** TS `Value` (string, numeric, boolean, null, enum, object and array values). */
export type Value = ValueShim;

const templatedStatementKinds = new Set([
  "ModelStatement",
  "ScalarStatement",
  "InterfaceStatement",
  "OperationStatement",
  "UnionStatement",
  "AliasStatement",
]);

export interface Program {
  checker: Checker;
  diagnostics: readonly Diagnostic[];
  reportDiagnostic(diagnostic: Diagnostic): void;
  reportDiagnostics(diagnostics: readonly Diagnostic[]): void;
  hasError(): boolean;
  getGlobalNamespaceType(): Namespace;
  /** TS `program.stateMap`: per-program decorator state keyed by state symbol (M11). */
  stateMap(key: symbol): Map<Type, any>;
  /** TS `program.stateSet` (M11). */
  stateSet(key: symbol): Set<Type>;
  readonly stateMaps: Map<symbol, Map<Type, unknown>>;
  readonly stateSets: Map<symbol, Set<Type>>;
  compilerOptions: Record<string, unknown>;
  /** TS `program.host` output side, set by `compile()` (M12). */
  host?: EmitterHost;
  /** TS `projectRoot`; also how typekit's `$(program)` recognizes a program. */
  projectRoot: string;
  _decoratorState: WeakMap<Type, Record<string, unknown>>;
  _typeCache: ReadonlyMap<number, Type>;
  /** Node shims by Rust node id (M11). */
  _nodes: ReadonlyMap<number, NodeShim>;
  /** Synthetic type of an object / array decorator argument value (M11). */
  _createCompositeValueType(value: CompositeValue): Type;
  /** Runs a synthesized type's decorators on `checker.finishType` (installed by `runDecorators`). */
  _applyDecoratorsToType?: (type: Type) => void;
  /** TS `program.resolveTypeReference`, limited to plain dotted references (M11). */
  resolveTypeReference(reference: string): [Type | undefined, readonly Diagnostic[]];
  /** TS `program.sourceFiles`: loaded TypeSpec scripts by path (script shims, M12). */
  readonly sourceFiles: Map<string, SourceFileShim>;
  /** TS `program.jsSourceFiles`: loaded JS files by path (M12). */
  readonly jsSourceFiles: Map<string, SourceFileShim>;
}

/** TS `TypeSpecScriptNode` / `JsSourceFileNode` shim: the file path only (M12). */
export interface SourceFileShim {
  kind: number;
  file: { path: string };
}

/** Subset of the TS `Checker` used by the standard decorator implementations (M11). */
export interface Checker {
  getGlobalNamespaceType(): Namespace;
  getStdType(name: string): Type;
  isStdType(type: Type, name?: string): boolean;
  /** TS `checker.getTypeName` (M12). */
  getTypeName(type: Type, options?: TypeNameOptions): string;
  /** TS `checker.cloneType`: a finished copy with its own member collections (M12). */
  cloneType<T extends Type>(type: T, additionalProps?: Partial<T>): T;
  /** TS `checker.createAndFinishType` (M12). */
  createAndFinishType<T extends object>(typeDef: T): T & Type;
  /** TS `createType`, used by typekits to synthesize types after checking. */
  createType<T extends object>(typeDef: T): T & Type;
  /** TS `finishType`: runs the type's decorators and marks it finished. */
  finishType<T extends Type>(type: T): T;
  /** Returns `[related, diagnostics]` like TS; `source` may be a type or value. */
  isTypeAssignableTo(
    source: unknown,
    target: Type,
    diagnosticTarget: unknown,
  ): [boolean, Diagnostic[]];
  isValueOfType(source: unknown, target: Type, diagnosticTarget: unknown): [boolean, Diagnostic[]];
  createLiteralType(value: string | number | boolean | { toString(): string }): Type;
  /** Not tracked by the Rust checker yet (M12): always `undefined`. */
  getValueExactType(value: unknown): Type | undefined;
  /** Plain dotted references only; template arguments are not supported (M12). */
  resolveTypeReference(reference: string): [Type | undefined, readonly Diagnostic[]];
  readonly errorType: Type;
  readonly voidType: Type;
  readonly neverType: Type;
  readonly nullType: Type;
  readonly anyType: Type;
}

/** TS `Node` shim: kind (TS `SyntaxKind` value), location and parent (M11). */
export interface NodeShim {
  readonly kind: number | undefined;
  readonly syntaxKind: string;
  readonly nodeId: number;
  readonly pos: number;
  readonly end: number;
  readonly file?: string;
  /** Span of the node's identifier (TS `getSourceLocation(node, { locateId: true })`, M13). */
  readonly idPos?: number;
  readonly idEnd?: number;
  parent?: NodeShim;
}

export interface DecoratorArgument {
  value: unknown;
  jsValue: unknown;
  node?: NodeShim;
}

export interface DecoratorApplication {
  /** JS implementation (M11) or `#<nodeId>` placeholder when decorators did not run. */
  decorator: string | ((...args: any[]) => unknown);
  /** Fully qualified decorator name, e.g. `TypeSpec.@doc` (M11). */
  decoratorName?: string;
  args: unknown[] | DecoratorArgument[];
  node?: NodeShim;
}

/** TS `TypeMapper` of a template instance (M12). */
export interface TypeMapper {
  partial: boolean;
  args: Type[];
  map: Map<TemplateParameter, Type>;
  source: { node: NodeShim };
}

interface BaseType {
  readonly id: number;
  readonly entityKind: "Type";
  readonly serializedKind: string;
  kind: string;
  decorators: DecoratorApplication[];
  isFinished: boolean;
  node?: NodeShim;
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
  templateMapper?: TypeMapper;
  isTemplateDeclaration: boolean;
}

export interface ModelProperty extends BaseType {
  kind: "ModelProperty";
  name: string;
  type: Type;
  sourceProperty?: ModelProperty;
  optional: boolean;
  /** @deprecated TS keeps the type form for compatibility; use defaultValue. */
  default?: Type;
  defaultValue?: Value;
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
  templateMapper?: TypeMapper;
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
  templateMapper?: TypeMapper;
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
  templateMapper?: TypeMapper;
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
  templateMapper?: TypeMapper;
  isTemplateDeclaration: boolean;
}

export interface Alias extends BaseType {
  kind: "Alias";
  name: string;
  namespace?: Namespace;
  target: Type;
  templateArguments?: Type[];
  templateParameters?: TemplateParameter[];
  templateMapper?: TypeMapper;
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

export interface Tuple extends BaseType {
  kind: "Tuple";
  values: Type[];
}

export interface StringTemplate extends BaseType {
  kind: "StringTemplate";
  spans: StringTemplateSpan[];
  stringValue?: string;
}

export interface StringTemplateSpan extends BaseType {
  kind: "StringTemplateSpan";
  isInterpolated: boolean;
  type: Type;
}

/** Object / array value shapes produced by the decorator argument marshalling (M11). */
export type CompositeValue =
  | { valueKind: "ObjectValue"; properties: Map<string, { value: unknown }> }
  | { valueKind: "ArrayValue"; values: unknown[] };

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
  | TemplateParameter
  | Tuple
  | StringTemplate
  | StringTemplateSpan;

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

export function createProgram(checkResult: CheckResultJs | CheckProgramResultJs): Program {
  const serializedById = new Map(checkResult.types.map((type) => [type.id, type]));
  const typeCache = new Map<number, Type>();
  const globalNamespaceId = findGlobalNamespaceId(checkResult.types);
  const decoratorState = new WeakMap<Type, Record<string, unknown>>();
  const diagnosticCollector = createDiagnosticCollector(
    checkResult.diagnostics as readonly SerializedDiagnostic[],
  );
  const nodeInfos: readonly NodeInfoJs[] = "nodes" in checkResult ? checkResult.nodes : [];
  const nodes = new Map<number, NodeShim>(
    nodeInfos.map((info) => [
      info.nodeId,
      {
        kind: syntaxKindValue(info.kind),
        syntaxKind: info.kind,
        nodeId: info.nodeId,
        pos: info.pos,
        end: info.end,
        file: info.file ?? undefined,
        ...(info.idPos !== undefined && info.idPos !== null
          ? { idPos: info.idPos, idEnd: info.idEnd ?? info.idPos }
          : {}),
        // Template declarations get their parameters in `linkTemplates`.
        ...(templatedStatementKinds.has(info.kind) ? { templateParameters: [] } : {}),
      },
    ]),
  );
  const relationHandle =
    "relationHandle" in checkResult ? (checkResult.relationHandle ?? undefined) : undefined;
  const stateMaps = new Map<symbol, Map<Type, unknown>>();
  const stateSets = new Map<symbol, Set<Type>>();
  const literalTypes = new Map<string, Type>();
  const compositeValueTypes = new WeakMap<Type, CompositeValue>();
  let compositeTypeCount = 0;

  const checker: Checker = {
    getGlobalNamespaceType: () => getOrCreate(globalNamespaceId) as Namespace,
    getStdType(name) {
      const typeSpec = (getOrCreate(globalNamespaceId) as Namespace).namespaces.get("TypeSpec");
      const type = typeSpec?.scalars.get(name) ?? typeSpec?.models.get(name);
      if (!type) throw new Error(`Standard type '${name}' is not loaded.`);
      return type;
    },
    isStdType(type, name) {
      if (!("namespace" in type) || type.namespace?.name !== "TypeSpec") return false;
      const namespace = type.namespace.namespace;
      if (namespace === undefined || namespace.name !== "" || namespace.namespace) return false;
      return (
        (type.kind === "Scalar" || type.kind === "Model") &&
        (name === undefined || type.name === name)
      );
    },
    isTypeAssignableTo: (source, target, diagnosticTarget) =>
      isAssignable(source, target, diagnosticTarget),
    isValueOfType: (source, target, diagnosticTarget) =>
      isAssignable(source, target, diagnosticTarget),
    createLiteralType,
    createType(typeDef) {
      return Object.assign(typeDef, { entityKind: "Type", isFinished: false }) as any;
    },
    createAndFinishType: (typeDef) => checker.finishType(checker.createType(typeDef)),
    getTypeName: (type, options) => compilerGetTypeName(type as any, options),
    cloneType(type, additionalProps) {
      const clone: any = { ...type, ...additionalProps };
      for (const [key, value] of Object.entries(clone)) {
        if (additionalProps && key in additionalProps) continue;
        if (value instanceof Map) clone[key] = new Map(value);
        else if (key === "decorators" && Array.isArray(value)) clone[key] = [...value];
      }
      return checker.finishType(clone);
    },
    finishType(type) {
      if ("decorators" in type && type.decorators.length > 0) {
        program._applyDecoratorsToType?.(type);
      }
      type.isFinished = true;
      return type;
    },
    getValueExactType: () => undefined,
    resolveTypeReference: (reference) => [resolveDottedReference(reference), []],
    get errorType() {
      return intrinsic("ErrorType");
    },
    get voidType() {
      return intrinsic("void");
    },
    get neverType() {
      return intrinsic("never");
    },
    get nullType() {
      return intrinsic("null");
    },
    get anyType() {
      return intrinsic("unknown");
    },
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
    resolveTypeReference: checker.resolveTypeReference,
    stateMap(key) {
      let map = stateMaps.get(key);
      if (!map) stateMaps.set(key, (map = new Map()));
      return map as Map<Type, any>;
    },
    stateSet(key) {
      let set = stateSets.get(key);
      if (!set) stateSets.set(key, (set = new Set()));
      return set;
    },
    stateMaps,
    stateSets,
    compilerOptions: {},
    sourceFiles: sourceFileShims("typespec", compilerSyntaxKind.TypeSpecScript),
    jsSourceFiles: sourceFileShims("js", compilerSyntaxKind.JsSourceFile),
    projectRoot: "/",
    _decoratorState: decoratorState,
    _typeCache: typeCache,
    _nodes: nodes,
    _createCompositeValueType: createCompositeValueType,
  };

  getOrCreate(globalNamespaceId);
  for (const type of checkResult.types) {
    getOrCreate(type.id);
  }
  populateReverseLinks();
  linkTemplates();
  linkDefaultValues();
  populateDecoratorState(checkResult.decoratorState);
  if ("deprecations" in checkResult) {
    // TS `markDeprecated` state (`createStateSymbol("deprecated")`), read by `getDeprecationDetails`.
    const deprecated = program.stateMap(Symbol.for("TypeSpec.deprecated"));
    for (const { typeId, message } of checkResult.deprecations) {
      const type = typeCache.get(typeId);
      if (type) deprecated.set(type, { message });
    }
  }
  return program;

  function sourceFileShims(kind: string, syntaxKind: number): Map<string, SourceFileShim> {
    const files = "sourceFiles" in checkResult ? checkResult.sourceFiles : [];
    return new Map(
      files
        .filter((file) => file.kind === kind)
        .map((file) => [file.path, { kind: syntaxKind, file: { path: file.path } }]),
    );
  }

  /** TS `resolveTypeReference` for plain dotted references (no template arguments). */
  function resolveDottedReference(reference: string): Type | undefined {
    let current: Type | undefined = getOrCreate(globalNamespaceId);
    for (const segment of reference.split(".")) {
      const name = segment.trim();
      const t: any = current;
      switch (t?.kind) {
        case "Namespace":
          current = [
            t.namespaces,
            t.models,
            t.scalars,
            t.enums,
            t.unions,
            t.interfaces,
            t.operations,
          ]
            .find((members: Map<string, Type>) => members.has(name))
            ?.get(name);
          break;
        case "Enum":
          current = t.members.get(name);
          break;
        case "Model":
          current = t.properties.get(name);
          break;
        case "Interface":
          current = t.operations.get(name);
          break;
        case "Union":
          current = t.variants.get(name);
          break;
        default:
          return undefined;
      }
    }
    return current;
  }

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
    const node = serialized.nodeId === undefined ? undefined : nodes.get(serialized.nodeId);
    if (node) shell.node = node;
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
      case "Tuple":
        return { ...base, kind: "Tuple", values: [] };
      case "StringTemplate":
        return {
          ...base,
          kind: "StringTemplate",
          spans: [],
          ...(serialized.stringValue !== undefined && serialized.stringValue !== null
            ? { stringValue: serialized.stringValue }
            : {}),
        };
      case "StringTemplateSpan":
        return {
          ...base,
          kind: "StringTemplateSpan",
          isInterpolated: serialized.isInterpolated ?? false,
          type: undefinedTypePlaceholder(),
        };
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
        type.default = optionalType(serialized.defaultTypeId);
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
      case "Tuple":
        type.values = serialized.valueIds.map((id) =>
          dereferenceAlias(requiredType(id, "tuple value")),
        );
        break;
      case "StringTemplate":
        type.spans = typeArray<StringTemplateSpan>(serialized.spanIds);
        break;
      case "StringTemplateSpan":
        type.type = dereferenceAlias(requiredType(serialized.spanTypeId, "string template span"));
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

  /**
   * TS template linkage (M12): declarations' nodes expose `templateParameters` (read by
   * `isTemplateDeclaration`) and instances get a `templateMapper` (read by emitters to name
   * instances and by `isTemplateInstance`). Instances whose arguments still reference template
   * parameters are partial, like TS instances checked inside a template declaration.
   */
  /** TS ModelProperty.defaultValue: the default materialized as a Value. */
  function linkDefaultValues(): void {
    for (const serialized of checkResult.types) {
      if (!serialized.defaultValueJson) continue;
      const property = typeCache.get(serialized.id) as ModelProperty | undefined;
      if (property?.kind !== "ModelProperty") continue;
      const value = createValue(program, JSON.parse(serialized.defaultValueJson), undefined);
      // TS `checkDefaultValue` types the default as the property type.
      if (value) property.defaultValue = { ...value, type: property.type } as Value;
    }
  }

  function linkTemplates(): void {
    for (const type of typeCache.values()) {
      if (!("templateParameters" in type) || !type.node) continue;
      const parameters = type.templateParameters ?? [];
      if (type.isTemplateDeclaration) {
        const node = type.node as NodeShim & { templateParameters?: NodeShim[] };
        if (!node.templateParameters?.length) {
          node.templateParameters = parameters.map((parameter) => parameter.node!);
        }
        continue;
      }
      const args = type.templateArguments ?? [];
      if (args.length === 0) continue;
      const map = new Map<TemplateParameter, Type>();
      parameters.forEach((parameter, index) => map.set(parameter, args[index]));
      const partial = args.some(referencesTemplateParameter);
      type.templateMapper = { partial, args, map, source: { node: type.node } };
      if (partial) type.isFinished = false;
    }
  }

  function referencesTemplateParameter(type: Type): boolean {
    if (type.kind === "TemplateParameter") return true;
    if ("templateMapper" in type && type.templateMapper) return type.templateMapper.partial;
    return false;
  }

  function populateDecoratorState(entries: readonly DecoratorStateEntry[]): void {
    for (const entry of entries) {
      const target = getOrCreate(entry.typeId);
      const state = decoratorState.get(target) ?? {};
      state[entry.key] = decoratorStateValue(entry);
      decoratorState.set(target, state);
    }
  }

  function intrinsic(name: Intrinsic["name"]): Type {
    for (const type of typeCache.values()) {
      if (type.kind === "Intrinsic" && type.name === name) return type;
    }
    throw new Error(`Intrinsic type '${name}' is not loaded.`);
  }

  function createLiteralType(value: string | number | boolean | { toString(): string }): Type {
    const key = `${typeof value}:${String(value)}`;
    let type = literalTypes.get(key);
    if (type) return type;
    const base = {
      id: -1 - literalTypes.size,
      entityKind: "Type" as const,
      serializedKind: "",
      decorators: [],
      isFinished: true,
    };
    if (typeof value === "string") {
      type = { ...base, serializedKind: "StringLiteral", kind: "String", value };
    } else if (typeof value === "boolean") {
      type = { ...base, serializedKind: "BooleanLiteral", kind: "Boolean", value };
    } else {
      const valueAsString = String(value);
      type = {
        ...base,
        serializedKind: "NumericLiteral",
        kind: "Number",
        value: Number(valueAsString),
        numericValue: typeof value === "number" ? { toString: () => valueAsString } : value,
        valueAsString,
      };
    }
    literalTypes.set(key, type);
    return type;
  }

  /** `isTypeAssignableTo` through the retained Rust type arena (`relationHandle`). */
  function isAssignable(
    source: unknown,
    target: Type,
    diagnosticTarget: unknown,
  ): [boolean, Diagnostic[]] {
    let sourceType = source as Type;
    if ((source as { entityKind?: string })?.entityKind === "Value") {
      const valueKind = (source as { valueKind: string }).valueKind;
      if (valueKind === "ObjectValue" || valueKind === "ArrayValue") {
        return isCompositeValueOfType(source as CompositeValue, target)
          ? [true, []]
          : [
              false,
              [
                {
                  code: "unassignable",
                  severity: "error",
                  message: `Type '${valueKind === "ObjectValue" ? "object value" : "array value"}' is not assignable to type '${(target as { name?: string }).name ?? target.kind}'`,
                  target: diagnosticTarget as Type | DiagnosticTarget,
                },
              ],
            ];
      }
      sourceType = (source as { type: Type }).type;
    }
    if (sourceType === target) return [true, []];
    const composite = compositeValueTypes.get(sourceType);
    if (composite) return isAssignable(composite, target, diagnosticTarget);
    const relationSource =
      sourceType.id >= 0 && typeCache.get(sourceType.id) === sourceType
        ? { typeId: sourceType.id }
        : sourceType.kind === "String"
          ? { string: sourceType.value }
          : sourceType.kind === "Number"
            ? { numeric: sourceType.valueAsString }
            : sourceType.kind === "Boolean"
              ? { boolean: sourceType.value }
              : undefined;
    if (
      relationHandle === undefined ||
      relationSource === undefined ||
      typeCache.get(target.id) !== target
    ) {
      return [false, []];
    }
    const result = isTypeAssignableToNative(relationHandle, relationSource, target.id);
    if (!result) return [false, []];
    return [
      result.related,
      result.diagnostics.map((diagnostic) => ({
        code: diagnostic.code,
        severity: "error",
        message: diagnostic.message,
        target: diagnosticTarget as Type | DiagnosticTarget,
      })),
    ];
  }

  /**
   * TS gives object / array values an anonymous model / tuple type; the adapter uses an empty
   * synthetic model mapped back to the value, so relating that type checks the value.
   */
  function createCompositeValueType(value: CompositeValue): Type {
    const type: Model = {
      id: -1_000_000 - compositeTypeCount++,
      entityKind: "Type",
      kind: "Model",
      serializedKind: "Model",
      name: "",
      decorators: [],
      isFinished: true,
      properties: new Map(),
      sourceModels: [],
      derivedModels: [],
      expression: true,
      isTemplateDeclaration: false,
    } as unknown as Model;
    compositeValueTypes.set(type, value);
    return type;
  }

  /**
   * Structural `isValueOfType` for object and array values (the Rust checker does not
   * validate them yet, M12): properties against the model's (inherited) properties or
   * indexer, items against the array element type.
   */
  function isCompositeValueOfType(value: CompositeValue, target: Type): boolean {
    while (target.kind === "Alias") target = target.target;
    switch (target.kind) {
      case "Intrinsic":
        return target.name === "unknown";
      case "Union":
        return [...target.variants.values()].some((variant) =>
          isEntityValueOfType(value, variant.type),
        );
      case "Model": {
        if (value.valueKind === "ArrayValue") {
          if (!target.indexer || target.indexer.key.name !== "integer") return false;
          const element = target.indexer.value;
          return value.values.every((item) => isEntityValueOfType(item, element));
        }
        if (target.indexer?.key.name === "integer") return false;
        const properties = new Map<string, ModelProperty>();
        for (let model: Model | undefined = target; model; model = model.baseModel) {
          for (const [name, property] of model.properties) {
            if (!properties.has(name)) properties.set(name, property);
          }
        }
        for (const [name, property] of properties) {
          if (!property.optional && !value.properties.has(name)) return false;
        }
        for (const [name, { value: propertyValue }] of value.properties) {
          const property = properties.get(name);
          const propertyType = property?.type ?? target.indexer?.value;
          if (!propertyType || !isEntityValueOfType(propertyValue, propertyType)) return false;
        }
        return true;
      }
      default:
        return false;
    }
  }

  function isEntityValueOfType(value: unknown, target: Type): boolean {
    return isAssignable(value, target, undefined)[0];
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

let syntaxKinds: Record<string, number> | undefined;

/** TS `SyntaxKind` value for a Rust syntax kind name (the enums use the same member names). */
function syntaxKindValue(name: string): number | undefined {
  syntaxKinds ??= compilerSyntaxKind as unknown as Record<string, number>;
  return syntaxKinds[name];
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
