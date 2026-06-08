import type {
  Alias,
  Enum,
  Interface,
  Model,
  ModelProperty,
  Namespace,
  Operation,
  Program,
  Scalar,
  Type,
  Union,
  UnionVariant,
} from "./program.js";
import { isTemplateDeclaration } from "./type-utils.js";

export const ListenerFlow = {
  NoRecursion: "skip",
} as const;

export type ListenerFlow = (typeof ListenerFlow)[keyof typeof ListenerFlow];

export interface NavigationOptions {
  includeTemplateDeclaration?: boolean;
  visitDerivedTypes?: boolean;
}

export interface SemanticNodeListener {
  root?(program: Program): void | ListenerFlow;
  namespace?(namespace: Namespace): void | ListenerFlow;
  exitNamespace?(namespace: Namespace): void;
  model?(model: Model): void | ListenerFlow;
  exitModel?(model: Model): void;
  modelProperty?(property: ModelProperty): void | ListenerFlow;
  exitModelProperty?(property: ModelProperty): void;
  scalar?(scalar: Scalar): void | ListenerFlow;
  exitScalar?(scalar: Scalar): void;
  enum?(type: Enum): void | ListenerFlow;
  exitEnum?(type: Enum): void;
  union?(type: Union): void | ListenerFlow;
  exitUnion?(type: Union): void;
  unionVariant?(type: UnionVariant): void | ListenerFlow;
  exitUnionVariant?(type: UnionVariant): void;
  operation?(operation: Operation): void | ListenerFlow;
  exitOperation?(operation: Operation): void;
  interface?(type: Interface): void | ListenerFlow;
  exitInterface?(type: Interface): void;
  alias?(type: Alias): void | ListenerFlow;
  exitAlias?(type: Alias): void;
}

interface NavigationContext {
  visited: Set<Type>;
  listeners: SemanticNodeListener;
  options: Required<NavigationOptions>;
}

export function navigateProgram(
  program: Program,
  listeners: SemanticNodeListener,
  options: NavigationOptions = {},
): void {
  const context = createContext(listeners, options);
  listeners.root?.(program);
  navigateNamespaceType(program.getGlobalNamespaceType(), context);
}

export function navigateType(
  type: Type,
  listeners: SemanticNodeListener,
  options: NavigationOptions = {},
): void {
  navigateTypeInternal(type, createContext(listeners, options));
}

function createContext(
  listeners: SemanticNodeListener,
  options: NavigationOptions,
): NavigationContext {
  return {
    visited: new Set(),
    listeners,
    options: {
      includeTemplateDeclaration: options.includeTemplateDeclaration ?? false,
      visitDerivedTypes: options.visitDerivedTypes ?? false,
    },
  };
}

function navigateNamespaceType(namespace: Namespace, context: NavigationContext): void {
  if (context.listeners.namespace?.(namespace) === ListenerFlow.NoRecursion) return;
  for (const model of namespace.models.values()) {
    navigateModelType(model, context);
  }
  for (const scalar of namespace.scalars.values()) {
    navigateScalarType(scalar, context);
  }
  for (const operation of namespace.operations.values()) {
    navigateOperationType(operation, context);
  }
  for (const subNamespace of namespace.namespaces.values()) {
    if (!(namespace.name === "TypeSpec" && subNamespace.name === "Prototypes")) {
      navigateNamespaceType(subNamespace, context);
    }
  }
  for (const union of namespace.unions.values()) {
    navigateUnionType(union, context);
  }
  for (const iface of namespace.interfaces.values()) {
    navigateInterfaceType(iface, context);
  }
  for (const enumType of namespace.enums.values()) {
    navigateEnumType(enumType, context);
  }
  for (const alias of namespace.aliases.values()) {
    navigateAliasType(alias, context);
  }
  context.listeners.exitNamespace?.(namespace);
}

function checkVisited(context: NavigationContext, type: Type): boolean {
  if (context.visited.has(type)) {
    return true;
  }
  context.visited.add(type);
  return false;
}

function shouldNavigateTemplatableType(
  context: NavigationContext,
  type: Model | Operation | Interface | Union | Scalar | Alias,
): boolean {
  if (context.options.includeTemplateDeclaration) {
    return type.isFinished || isTemplateDeclaration(type);
  }
  return type.isFinished;
}

function navigateOperationType(operation: Operation, context: NavigationContext): void {
  if (checkVisited(context, operation) || !shouldNavigateTemplatableType(context, operation)) {
    return;
  }
  if (context.listeners.operation?.(operation) === ListenerFlow.NoRecursion) return;
  for (const parameter of operation.parameters.properties.values()) {
    navigateTypeInternal(parameter, context);
  }
  navigateTypeInternal(operation.returnType, context);
  if (operation.sourceOperation) {
    navigateTypeInternal(operation.sourceOperation, context);
  }
  context.listeners.exitOperation?.(operation);
}

function navigateModelType(model: Model, context: NavigationContext): void {
  if (checkVisited(context, model) || !shouldNavigateTemplatableType(context, model)) {
    return;
  }
  if (context.listeners.model?.(model) === ListenerFlow.NoRecursion) return;
  for (const property of model.properties.values()) {
    navigateModelTypeProperty(property, context);
  }
  if (model.baseModel) {
    navigateModelType(model.baseModel, context);
  }
  if (model.indexer?.value) {
    navigateTypeInternal(model.indexer.value, context);
  }
  if (context.options.visitDerivedTypes) {
    for (const derived of model.derivedModels) {
      navigateModelType(derived, context);
    }
  }
  context.listeners.exitModel?.(model);
}

function navigateModelTypeProperty(property: ModelProperty, context: NavigationContext): void {
  if (checkVisited(context, property)) {
    return;
  }
  if (context.listeners.modelProperty?.(property) === ListenerFlow.NoRecursion) return;
  navigateTypeInternal(property.type, context);
  context.listeners.exitModelProperty?.(property);
}

function navigateScalarType(scalar: Scalar, context: NavigationContext): void {
  if (checkVisited(context, scalar) || !shouldNavigateTemplatableType(context, scalar)) {
    return;
  }
  if (context.listeners.scalar?.(scalar) === ListenerFlow.NoRecursion) return;
  if (scalar.baseScalar) {
    navigateScalarType(scalar.baseScalar, context);
  }
  context.listeners.exitScalar?.(scalar);
}

function navigateInterfaceType(type: Interface, context: NavigationContext): void {
  if (checkVisited(context, type) || !shouldNavigateTemplatableType(context, type)) {
    return;
  }
  if (context.listeners.interface?.(type) === ListenerFlow.NoRecursion) return;
  for (const op of type.operations.values()) {
    navigateOperationType(op, context);
  }
  context.listeners.exitInterface?.(type);
}

function navigateEnumType(type: Enum, context: NavigationContext): void {
  if (checkVisited(context, type)) {
    return;
  }
  if (context.listeners.enum?.(type) === ListenerFlow.NoRecursion) return;
  context.listeners.exitEnum?.(type);
}

function navigateUnionType(type: Union, context: NavigationContext): void {
  if (checkVisited(context, type) || !shouldNavigateTemplatableType(context, type)) {
    return;
  }
  if (context.listeners.union?.(type) === ListenerFlow.NoRecursion) return;
  for (const variant of type.variants.values()) {
    navigateUnionTypeVariant(variant, context);
  }
  context.listeners.exitUnion?.(type);
}

function navigateUnionTypeVariant(type: UnionVariant, context: NavigationContext): void {
  if (checkVisited(context, type)) {
    return;
  }
  if (context.listeners.unionVariant?.(type) === ListenerFlow.NoRecursion) return;
  navigateTypeInternal(type.type, context);
  context.listeners.exitUnionVariant?.(type);
}

function navigateAliasType(type: Alias, context: NavigationContext): void {
  if (checkVisited(context, type) || !shouldNavigateTemplatableType(context, type)) {
    return;
  }
  if (context.listeners.alias?.(type) === ListenerFlow.NoRecursion) return;
  navigateTypeInternal(type.target, context);
  context.listeners.exitAlias?.(type);
}

function navigateTypeInternal(type: Type, context: NavigationContext): void {
  switch (type.kind) {
    case "Model":
      return navigateModelType(type, context);
    case "Scalar":
      return navigateScalarType(type, context);
    case "ModelProperty":
      return navigateModelTypeProperty(type, context);
    case "Namespace":
      return navigateNamespaceType(type, context);
    case "Interface":
      return navigateInterfaceType(type, context);
    case "Enum":
      return navigateEnumType(type, context);
    case "Operation":
      return navigateOperationType(type, context);
    case "Union":
      return navigateUnionType(type, context);
    case "UnionVariant":
      return navigateUnionTypeVariant(type, context);
    case "Alias":
      return navigateAliasType(type, context);
    case "String":
    case "Number":
    case "Boolean":
    case "EnumMember":
    case "Intrinsic":
    case "TemplateParameter":
      return;
  }
}
