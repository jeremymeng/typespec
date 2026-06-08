import type { Type } from "./program.js";

export type DiagnosticSeverity = "error" | "warning" | "hint";

export interface DiagnosticTarget {
  pos: number;
  end: number;
}

export interface Diagnostic {
  code: string;
  severity: DiagnosticSeverity;
  message: string;
  target?: Type | DiagnosticTarget;
}

export interface SerializedDiagnostic {
  code: string;
  severity: string;
  message: string;
  pos: number;
  end: number;
}

export function createDiagnosticCollector(initial: readonly SerializedDiagnostic[] = []) {
  const diagnostics: Diagnostic[] = initial.map((diagnostic) => ({
    code: diagnostic.code,
    severity: normalizeSeverity(diagnostic.severity),
    message: diagnostic.message,
    target: { pos: diagnostic.pos, end: diagnostic.end },
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
