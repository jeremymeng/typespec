import type { Namespace, Type } from "./program.js";

export function getNamespaceFullName(namespace: Namespace): string {
  const names: string[] = [];
  let current: Namespace | undefined = namespace;
  while (current) {
    if (current.name !== "") {
      names.unshift(current.name);
    }
    current = current.namespace;
  }
  return names.join(".");
}

export function isTemplateDeclaration(type: Type): boolean {
  return "isTemplateDeclaration" in type && type.isTemplateDeclaration === true;
}

export function isType(value: unknown): value is Type {
  return (
    typeof value === "object" &&
    value !== null &&
    "entityKind" in value &&
    value.entityKind === "Type" &&
    "kind" in value &&
    typeof value.kind === "string"
  );
}

export function isErrorModel(type: Type): boolean {
  return type.kind === "Intrinsic" && type.name === "ErrorType";
}
