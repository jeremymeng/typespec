import {
  getDeprecated as rustGetDeprecated,
  getDoc as rustGetDoc,
  getKey as rustGetKey,
  getSummary as rustGetSummary,
} from "../../src/decorator-helpers.js";
import type { Program as RustProgram, Type as RustType } from "../../src/program.js";
import {
  navigateProgram as rustNavigateProgram,
  type SemanticNodeListener,
} from "../../src/walker.js";

export interface EmitterOutput {
  namespaces: Array<{ fullName: string; doc?: string; summary?: string; deprecated?: string }>;
  models: Array<{
    fullName: string;
    properties: Array<{ name: string; type: string; doc?: string; key?: string }>;
    baseModel?: string;
    doc?: string;
    summary?: string;
    deprecated?: string;
  }>;
  operations: Array<{
    fullName: string;
    parameters: Array<{ name: string; type: string }>;
    returnType: string;
    doc?: string;
    summary?: string;
    deprecated?: string;
  }>;
  interfaces: Array<{ fullName: string; operations: string[]; doc?: string; summary?: string }>;
  enums: Array<{
    fullName: string;
    members: Array<{ name: string; value?: string | number | bigint; doc?: string }>;
    doc?: string;
  }>;
  unions: Array<{
    fullName: string;
    variants: Array<{ name: string; type: string; doc?: string }>;
    doc?: string;
  }>;
}

export interface EmitterRuntime<TProgram> {
  navigateProgram(program: TProgram, listener: SemanticNodeListener): void;
  getDoc(program: TProgram, target: EmitterType): string | undefined;
  getSummary(program: TProgram, target: EmitterType): string | undefined;
  getDeprecated(program: TProgram, target: EmitterType): string | undefined;
  getKey(program: TProgram, target: EmitterType): string | undefined;
}

export const rustEmitterRuntime: EmitterRuntime<RustProgram> = {
  navigateProgram: rustNavigateProgram,
  getDoc: (program, target) => rustGetDoc(program, target as unknown as RustType),
  getSummary: (program, target) => rustGetSummary(program, target as unknown as RustType),
  getDeprecated: (program, target) => rustGetDeprecated(program, target as unknown as RustType),
  getKey: (program, target) => rustGetKey(program, target as unknown as RustType),
};

export function runTestEmitter<TProgram>(
  program: TProgram,
  runtime: EmitterRuntime<TProgram>,
): EmitterOutput {
  const output: EmitterOutput = {
    namespaces: [],
    models: [],
    operations: [],
    interfaces: [],
    enums: [],
    unions: [],
  };

  runtime.navigateProgram(program, {
    namespace(namespace) {
      if (isTypeSpecNamespace(namespace)) return;
      output.namespaces.push({
        fullName: getNamespaceFullName(namespace),
        ...decoratorFields(program, runtime, namespace, ["doc", "summary", "deprecated"]),
      });
    },
    model(model) {
      if (model.name === "" || isTypeSpecType(model)) return;
      output.models.push({
        fullName: getTypeFullName(model),
        properties: [...model.properties.values()].map((property) => ({
          name: property.name,
          type: typeReference(property.type),
          ...decoratorFields(program, runtime, property, ["doc", "key"]),
        })),
        baseModel: model.baseModel ? getTypeFullName(model.baseModel) : undefined,
        ...decoratorFields(program, runtime, model, ["doc", "summary", "deprecated"]),
      });
    },
    operation(operation) {
      if (isTypeSpecType(operation)) return;
      output.operations.push({
        fullName: getOperationFullName(operation),
        parameters: [...operation.parameters.properties.values()].map((parameter) => ({
          name: parameter.name,
          type: typeReference(parameter.type),
        })),
        returnType: typeReference(operation.returnType),
        ...decoratorFields(program, runtime, operation, ["doc", "summary", "deprecated"]),
      });
    },
    interface(iface) {
      if (isTypeSpecType(iface)) return;
      output.interfaces.push({
        fullName: getTypeFullName(iface),
        operations: [...iface.operations.values()].map((operation) => operation.name),
        ...decoratorFields(program, runtime, iface, ["doc", "summary"]),
      });
    },
    enum(en) {
      if (isTypeSpecType(en)) return;
      output.enums.push({
        fullName: getTypeFullName(en),
        members: [...en.members.values()].map((member) => ({
          name: member.name,
          value: member.value,
          ...decoratorFields(program, runtime, member, ["doc"]),
        })),
        ...decoratorFields(program, runtime, en, ["doc"]),
      });
    },
    union(union) {
      if (isTypeSpecType(union)) return;
      output.unions.push({
        fullName: getTypeFullName(union),
        variants: [...union.variants.values()].map((variant) => ({
          name: String(variant.name),
          type: typeReference(variant.type),
          ...decoratorFields(program, runtime, variant, ["doc"]),
        })),
        ...decoratorFields(program, runtime, union, ["doc"]),
      });
    },
  });

  return normalizeOutput(output);
}

export function traceNavigation<TProgram>(
  program: TProgram,
  runtime: Pick<EmitterRuntime<TProgram>, "navigateProgram">,
): string[] {
  const events: string[] = [];
  runtime.navigateProgram(program, {
    namespace(namespace) {
      if (!isTypeSpecNamespace(namespace))
        events.push(`namespace:${getNamespaceFullName(namespace)}`);
    },
    exitNamespace(namespace) {
      if (!isTypeSpecNamespace(namespace)) {
        events.push(`exitNamespace:${getNamespaceFullName(namespace)}`);
      }
    },
    model(model) {
      if (model.name !== "" && !isTypeSpecType(model))
        events.push(`model:${getTypeFullName(model)}`);
    },
    exitModel(model) {
      if (model.name !== "" && !isTypeSpecType(model)) {
        events.push(`exitModel:${getTypeFullName(model)}`);
      }
    },
    modelProperty(property) {
      if (!isTypeSpecModelProperty(property)) {
        events.push(
          `modelProperty:${property.model ? getTypeFullName(property.model) : ""}.${property.name}`,
        );
      }
    },
    operation(operation) {
      if (!isTypeSpecType(operation)) events.push(`operation:${getOperationFullName(operation)}`);
    },
    interface(iface) {
      if (!isTypeSpecType(iface)) events.push(`interface:${getTypeFullName(iface)}`);
    },
    enum(en) {
      if (!isTypeSpecType(en)) events.push(`enum:${getTypeFullName(en)}`);
    },
    union(union) {
      if (!isTypeSpecType(union)) events.push(`union:${getTypeFullName(union)}`);
    },
    unionVariant(variant) {
      if (!isTypeSpecUnionVariant(variant)) {
        events.push(`unionVariant:${getTypeFullName(variant.union)}.${String(variant.name)}`);
      }
    },
  });
  return events;
}

type DecoratorField = "doc" | "summary" | "deprecated" | "key";

function decoratorFields<TProgram>(
  program: TProgram,
  runtime: EmitterRuntime<TProgram>,
  target: EmitterType,
  fields: readonly DecoratorField[],
): Partial<Record<DecoratorField, string>> {
  const result: Partial<Record<DecoratorField, string>> = {};
  for (const field of fields) {
    const value =
      field === "doc"
        ? runtime.getDoc(program, target)
        : field === "summary"
          ? runtime.getSummary(program, target)
          : field === "deprecated"
            ? runtime.getDeprecated(program, target)
            : runtime.getKey(program, target);
    if (value !== undefined) {
      result[field] = value;
    }
  }
  return result;
}

function normalizeOutput(output: EmitterOutput): EmitterOutput {
  return JSON.parse(
    JSON.stringify(output, (_key, value: unknown) =>
      typeof value === "bigint" ? value.toString() : value,
    ),
  ) as EmitterOutput;
}

function getOperationFullName(operation: EmitterOperation): string {
  if (operation.interface) {
    return `${getTypeFullName(operation.interface)}.${operation.name}`;
  }
  return qualify(operation.namespace, operation.name);
}

function getTypeFullName(type: EmitterNamedType): string {
  return qualify(type.namespace, typeDisplayName(type));
}

function qualify(namespace: EmitterNamespace | undefined, name: string): string {
  const namespaceName = namespace ? getNamespaceFullName(namespace) : "";
  return namespaceName ? `${namespaceName}.${name}` : name;
}

function getNamespaceFullName(namespace: EmitterNamespace): string {
  const names: string[] = [];
  let current: EmitterNamespace | undefined = namespace;
  while (current) {
    if (current.name !== "") {
      names.unshift(current.name);
    }
    current = current.namespace;
  }
  return names.join(".");
}

function typeReference(type: EmitterType): string {
  switch (type.kind) {
    case "Model":
    case "Enum":
    case "Union":
    case "Interface":
    case "Alias":
      return getTypeFullName(type);
    case "Scalar":
      return isTypeSpecType(type) ? type.name : getTypeFullName(type);
    case "ModelProperty":
      return `${type.model ? getTypeFullName(type.model) : ""}.${type.name}`;
    case "EnumMember":
      return `${getTypeFullName(type.enum)}.${type.name}`;
    case "UnionVariant":
      return `${getTypeFullName(type.union)}.${String(type.name)}`;
    case "String":
      return `"${type.value}"`;
    case "Number":
      return type.valueAsString;
    case "Boolean":
      return String(type.value);
    case "Intrinsic":
      return type.name;
    case "Namespace":
      return getNamespaceFullName(type);
    case "TemplateParameter":
      return type.name;
    default:
      return type.kind;
  }
}

function typeDisplayName(type: EmitterNamedType): string {
  const args = templateArguments(type);
  return args.length === 0 ? type.name : `${type.name}<${args.map(typeReference).join(", ")}>`;
}

function templateArguments(type: EmitterNamedType): EmitterType[] {
  if ("templateArguments" in type && type.templateArguments) {
    return type.templateArguments;
  }
  if ("templateMapper" in type && type.templateMapper) {
    return type.templateMapper.args;
  }
  return [];
}

function isTypeSpecType(type: EmitterType): boolean {
  return Boolean("namespace" in type && type.namespace && isTypeSpecNamespace(type.namespace));
}

function isTypeSpecModelProperty(property: EmitterModelProperty): boolean {
  return Boolean(property.model && isTypeSpecType(property.model));
}

function isTypeSpecUnionVariant(variant: EmitterUnionVariant): boolean {
  return isTypeSpecType(variant.union);
}

function isTypeSpecNamespace(namespace: EmitterNamespace): boolean {
  let current: EmitterNamespace | undefined = namespace;
  while (current) {
    if (current.name === "TypeSpec") {
      return true;
    }
    current = current.namespace;
  }
  return false;
}

interface EmitterBase {
  kind: string;
  name?: string | symbol;
  namespace?: EmitterNamespace;
}

interface EmitterNamespace extends EmitterBase {
  kind: "Namespace";
  name: string;
  namespace?: EmitterNamespace;
  models: Map<string, EmitterModel>;
  scalars: Map<string, EmitterScalar>;
  operations: Map<string, EmitterOperation>;
  namespaces: Map<string, EmitterNamespace>;
  interfaces: Map<string, EmitterInterface>;
  enums: Map<string, EmitterEnum>;
  unions: Map<string, EmitterUnion>;
}

interface EmitterModel extends EmitterBase {
  kind: "Model";
  name: string;
  properties: Map<string, EmitterModelProperty>;
  baseModel?: EmitterModel;
  templateArguments?: EmitterType[];
  templateMapper?: { args: EmitterType[] };
}

interface EmitterModelProperty extends EmitterBase {
  kind: "ModelProperty";
  name: string;
  model?: EmitterModel;
  type: EmitterType;
}

interface EmitterScalar extends EmitterBase {
  kind: "Scalar";
  name: string;
  templateArguments?: EmitterType[];
  templateMapper?: { args: EmitterType[] };
}

interface EmitterEnum extends EmitterBase {
  kind: "Enum";
  name: string;
  members: Map<string, EmitterEnumMember>;
}

interface EmitterEnumMember extends EmitterBase {
  kind: "EnumMember";
  name: string;
  enum: EmitterEnum;
  value?: string | number | bigint;
}

interface EmitterUnion extends EmitterBase {
  kind: "Union";
  name: string;
  variants: Map<string | symbol, EmitterUnionVariant>;
  templateArguments?: EmitterType[];
  templateMapper?: { args: EmitterType[] };
}

interface EmitterUnionVariant extends EmitterBase {
  kind: "UnionVariant";
  name: string | symbol;
  union: EmitterUnion;
  type: EmitterType;
}

interface EmitterOperation extends EmitterBase {
  kind: "Operation";
  name: string;
  interface?: EmitterInterface;
  parameters: EmitterModel;
  returnType: EmitterType;
  templateArguments?: EmitterType[];
  templateMapper?: { args: EmitterType[] };
}

interface EmitterInterface extends EmitterBase {
  kind: "Interface";
  name: string;
  operations: Map<string, EmitterOperation>;
  templateArguments?: EmitterType[];
  templateMapper?: { args: EmitterType[] };
}

interface EmitterAlias extends EmitterBase {
  kind: "Alias";
  name: string;
  target: EmitterType;
  templateArguments?: EmitterType[];
  templateMapper?: { args: EmitterType[] };
}

interface EmitterStringLiteral extends EmitterBase {
  kind: "String";
  value: string;
}

interface EmitterNumberLiteral extends EmitterBase {
  kind: "Number";
  valueAsString: string;
}

interface EmitterBooleanLiteral extends EmitterBase {
  kind: "Boolean";
  value: boolean;
}

interface EmitterIntrinsic extends EmitterBase {
  kind: "Intrinsic";
  name: string;
}

interface EmitterTemplateParameter extends EmitterBase {
  kind: "TemplateParameter";
  name: string;
}

type EmitterNamedType =
  EmitterModel | EmitterScalar | EmitterEnum | EmitterUnion | EmitterInterface | EmitterAlias;

type EmitterType =
  | EmitterNamespace
  | EmitterNamedType
  | EmitterModelProperty
  | EmitterEnumMember
  | EmitterUnionVariant
  | EmitterOperation
  | EmitterStringLiteral
  | EmitterNumberLiteral
  | EmitterBooleanLiteral
  | EmitterIntrinsic
  | EmitterTemplateParameter;
