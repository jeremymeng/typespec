import type { Type } from "./program.js";

export type DiagnosticSeverity = "error" | "warning" | "hint";

export interface DiagnosticTarget {
  /** Absolute file path (multi-file programs). */
  file?: string;
  pos: number;
  end: number;
}

export interface Diagnostic {
  code: string;
  severity: DiagnosticSeverity;
  message: string;
  /** A type, node shim, location, or TS `NoTarget` (symbol) for decorator diagnostics. */
  target?: Type | DiagnosticTarget | symbol;
}

export interface SerializedDiagnostic {
  code: string;
  severity: string;
  message: string;
  file?: string | null;
  pos: number;
  end: number;
}

/**
 * Location of a diagnostic target like TS `getSourceLocation`: types resolve to their
 * declaration node; `undefined` for targetless diagnostics.
 */
export function getDiagnosticLocation(target: Diagnostic["target"]): DiagnosticTarget | undefined {
  if (target === undefined || typeof target === "symbol") return undefined;
  if ("entityKind" in target) {
    const node = (target as { node?: DiagnosticTarget }).node;
    return node && { file: node.file, pos: node.pos, end: node.end };
  }
  if ("kind" in target && "node" in target) {
    // Values and other entities carrying a node.
    const node = (target as { node?: DiagnosticTarget }).node;
    return node && { file: node.file, pos: node.pos, end: node.end };
  }
  return { file: target.file, pos: target.pos, end: target.end };
}

export function createDiagnosticCollector(initial: readonly SerializedDiagnostic[] = []) {
  const diagnostics: Diagnostic[] = initial.map((diagnostic) => ({
    code: diagnostic.code,
    severity: normalizeSeverity(diagnostic.severity),
    message: diagnostic.message,
    target: {
      ...(diagnostic.file ? { file: diagnostic.file } : {}),
      pos: diagnostic.pos,
      end: diagnostic.end,
    },
  }));

  return {
    diagnostics,
    reportDiagnostic(diagnostic: Diagnostic): void {
      diagnostics.push(diagnostic);
    },
  };
}

function normalizeSeverity(severity: string): DiagnosticSeverity {
  switch (severity) {
    case "error":
    case "warning":
    case "hint":
      return severity;
    default:
      return "error";
  }
}
