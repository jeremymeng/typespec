import type { Program, Type } from "./program.js";

export function getDoc(program: Program, target: Type): string | undefined {
  return getStringState(program, target, "@doc:doc");
}

export function getSummary(program: Program, target: Type): string | undefined {
  return getStringState(program, target, "@summary:summary");
}

export function getDeprecated(program: Program, target: Type): string | undefined {
  return getStringState(program, target, "@deprecated:deprecated");
}

export function getKey(program: Program, target: Type): string | undefined {
  return getStringState(program, target, "@key:key");
}

export const getKeyName = getKey;

function getStringState(program: Program, target: Type, key: string): string | undefined {
  const value = program._decoratorState.get(target)?.[key];
  return typeof value === "string" ? value : undefined;
}
