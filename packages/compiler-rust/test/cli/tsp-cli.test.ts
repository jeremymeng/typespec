// M13: `tsp compile --use-rust-checker` end to end. Each fixture is compiled by the real TS CLI
// (packages/compiler/entrypoints/cli.js) with and without the flag; the emitted files, the
// normalized console output (diagnostics included) and the exit code must match.

import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, rmSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const here = dirname(fileURLToPath(import.meta.url));
const fixturesRoot = join(here, "fixtures");
const cliPath = resolve(here, "../../../compiler/entrypoints/cli.js");

interface CliResult {
  exitCode: number | null;
  stdout: string;
  stderr: string;
  files: Record<string, string>;
}

function runCli(
  fixture: string,
  mode: "ts" | "rust",
  extraArgs: string[] = [],
  env: Record<string, string> = {},
): CliResult {
  const cwd = join(fixturesRoot, fixture);
  const outputDir = join(cwd, "tsp-output", mode);
  rmSync(outputDir, { recursive: true, force: true });
  const args = [
    cliPath,
    "compile",
    ".",
    "--emit",
    "@typespec/json-schema",
    "--output-dir",
    outputDir,
    ...(mode === "rust" && !env.TYPESPEC_USE_RUST_CHECKER ? ["--use-rust-checker"] : []),
    ...extraArgs,
  ];
  const result = spawnSync(process.execPath, args, {
    cwd,
    encoding: "utf8",
    env: { ...process.env, TYPESPEC_USE_RUST_CHECKER: "", ...env, NO_COLOR: "1" },
  });
  const files = readTree(outputDir);
  rmSync(join(cwd, "tsp-output"), { recursive: true, force: true });
  return {
    exitCode: result.status,
    stdout: normalizeOutput(result.stdout, mode),
    stderr: result.stderr,
    files,
  };
}

function normalizeOutput(output: string, mode: string): string {
  return output
    .replace(/\r\n/g, "\n")
    .replaceAll(`tsp-output/${mode}`, "tsp-output/<mode>")
    .replaceAll(`tsp-output\\${mode}`, "tsp-output/<mode>")
    .replace(/\d+ms/g, "<n>ms");
}

function readTree(root: string): Record<string, string> {
  const files: Record<string, string> = {};
  if (!existsSync(root)) return files;
  const visit = (dir: string) => {
    for (const name of readdirSync(dir)) {
      const path = join(dir, name);
      if (statSync(path).isDirectory()) visit(path);
      else files[relative(root, path).replace(/\\/g, "/")] = readFileSync(path, "utf8");
    }
  };
  visit(root);
  return files;
}

function expectSameResult(fixture: string, extraArgs: string[] = []) {
  const ts = runCli(fixture, "ts", extraArgs);
  const rust = runCli(fixture, "rust", extraArgs);
  expect(rust.stderr).not.toMatch(/Internal compiler error|Error \[ERR_/);
  expect(rust.stdout).toEqual(ts.stdout);
  expect(rust.exitCode).toEqual(ts.exitCode);
  expect(rust.files).toEqual(ts.files);
  return { ts, rust };
}

describe("tsp compile --use-rust-checker", { timeout: 120_000 }, () => {
  it("emits json-schema output identical to the TS checker", () => {
    const { rust } = expectSameResult("json-schema");
    expect(rust.exitCode).toBe(0);
    expect(Object.keys(rust.files).length).toBeGreaterThan(0);
    expect(rust.stdout).toContain("Compilation completed successfully.");
  });

  it("reports warnings, suppressions and duplicate suppressions like TS", () => {
    const { rust } = expectSameResult("warnings");
    expect(rust.stdout).toContain("warning deprecated");
    expect(rust.stdout).toContain("warning duplicate-suppression");
    expect(Object.keys(rust.files).length).toBeGreaterThan(0);
  });

  it("reports errors with line and column, and does not emit", () => {
    const { rust } = expectSameResult("errors");
    expect(rust.exitCode).toBe(1);
    expect(rust.stdout).toMatch(/main\.tsp:8:10 - error invalid-ref/);
    expect(rust.stdout).toContain("error suppress-error");
    expect(rust.files).toEqual({});
  });

  it("gates built-in linter rules by the configured rule set", () => {
    const { rust } = expectSameResult("linter");
    expect(rust.stdout).toContain("warning @typespec/compiler/unused-using");
    expect(rust.stdout).toContain("warning @typespec/compiler/unused-template-parameter");
  });

  it("honors --linter-rule overrides and --warn-as-error", () => {
    expectSameResult("linter", ["--linter-rule", "compiler/unused-using=false"]);
    const { rust } = expectSameResult("linter", ["--warn-as-error"]);
    expect(rust.exitCode).toBe(1);
  });

  it("does not run built-in linter rules without a rule set", () => {
    const { rust } = expectSameResult("json-schema", ["--no-linter"]);
    expect(rust.stdout).not.toContain("unused-");
  });

  it("resolves package.json #imports", () => {
    const { rust } = expectSameResult("imports");
    expect(rust.exitCode).toBe(0);
  });

  it("uses the tspconfig.yaml entrypoint", () => {
    const { rust } = expectSameResult("entrypoint");
    expect(rust.exitCode).toBe(0);
    expect(Object.keys(rust.files)).toContain("@typespec/json-schema/Widget.yaml");
  });

  it("enables compiler features from tspconfig.yaml", () => {
    const { rust } = expectSameResult("features");
    expect(rust.stdout).not.toContain("experimental-feature");
  });

  it("is enabled by the TYPESPEC_USE_RUST_CHECKER environment variable", () => {
    const ts = runCli("json-schema", "ts");
    const rust = runCli("json-schema", "rust", [], { TYPESPEC_USE_RUST_CHECKER: "1" });
    expect(rust.stderr).toContain("Rust checker");
    expect(rust.stdout).toEqual(ts.stdout);
    expect(rust.files).toEqual(ts.files);
  });

  it("does not use the Rust checker by default", () => {
    const ts = runCli("json-schema", "ts");
    expect(ts.stderr).not.toContain("Rust checker");
  });
});
