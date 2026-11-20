import * as compilerLib from "@typespec/compiler";
import { resolvePath } from "@typespec/compiler";
import { createTester, resolveVirtualPath } from "@typespec/compiler/testing";
import * as httpLib from "@typespec/http";
import { describe, expect, it } from "vitest";
import { compile } from "../../src/decorator-registration.js";
import { getDiagnosticLocation } from "../../src/diagnostics.js";
import { createVirtualProgramHost } from "../../src/host.js";

// M11: the real `@typespec/compiler` / `@typespec/http` decorator implementations run on the
// Rust-checked program; their getters read the resulting state through the Program adapter.

const packageRoot = resolvePath(import.meta.dirname, "../..");
const Tester = createTester(packageRoot, { libraries: ["@typespec/http"] });
const mainPath = resolveVirtualPath("main.tsp");

interface NormalizedDiagnostic {
  code: string;
  message: string;
  pos?: number;
  end?: number;
}

function normalizeCode(code: string) {
  return code.replace(/^@typespec\/compiler\//, "");
}

function sortDiagnostics(diagnostics: NormalizedDiagnostic[]) {
  return [...diagnostics].sort(
    (a, b) =>
      (a.pos ?? -1) - (b.pos ?? -1) ||
      a.code.localeCompare(b.code) ||
      a.message.localeCompare(b.message),
  );
}

async function compileTs(source: string) {
  const [{ program }, diagnostics] = await Tester.compileAndDiagnose(source);
  return {
    program,
    diagnostics: sortDiagnostics(
      diagnostics.map((d) => {
        const location = compilerLib.getSourceLocation(d.target as any);
        return {
          code: normalizeCode(d.code),
          message: d.message,
          ...(location && !location.isSynthetic && location.file.path === mainPath
            ? { pos: location.pos, end: location.end }
            : {}),
        };
      }),
    ),
  };
}

async function compileRust(source: string) {
  const program = await compile(mainPath, createVirtualProgramHost({ [mainPath]: source }), {
    libraries: ["@typespec/http"],
  });
  return {
    program,
    diagnostics: sortDiagnostics(
      program.diagnostics.map((d) => {
        const location = getDiagnosticLocation(d.target);
        return {
          code: normalizeCode(d.code),
          message: d.message,
          ...(location && (location.file ?? mainPath) === mainPath
            ? { pos: location.pos, end: location.end }
            : {}),
        };
      }),
    ),
  };
}

async function expectSameDiagnostics(source: string) {
  const [ts, rust] = await Promise.all([compileTs(source), compileRust(source)]);
  expect(rust.diagnostics).toEqual(ts.diagnostics);
  return { ts, rust };
}

/** JSON-friendly description of getter results shared by TS and adapter entities. */
function describe_(value: unknown, depth = 0): unknown {
  if (value === undefined || value === null) return value;
  if (typeof value !== "object") return value;
  if (value instanceof Map) {
    return [...value].map(([k, v]) => [describe_(k, depth + 1), describe_(v, depth + 1)]);
  }
  if (Array.isArray(value)) return value.map((v) => describe_(v, depth + 1));
  const entity = value as any;
  if (entity.entityKind === "Type") {
    const parts = [entity.name];
    for (
      let ns = entity.namespace ?? entity.model ?? entity.enum ?? entity.interface;
      ns;
      ns = ns.namespace
    ) {
      if (ns.name === "") break;
      parts.unshift(ns.name);
    }
    return entity.name === undefined ? { kind: entity.kind } : `${entity.kind}:${parts.join(".")}`;
  }
  if (entity.entityKind === "Value") {
    return { valueKind: entity.valueKind, value: describe_(entity.value, depth + 1) };
  }
  if (typeof entity.asNumber === "function") return `Numeric:${entity.toString()}`;
  if (depth > 4) return "...";
  return Object.fromEntries(
    Object.entries(entity)
      .filter(([key]) => key !== "node" && key !== "decorator")
      .map(([key, v]) => [key, describe_(v, depth + 1)]),
  );
}

function find(program: any, path: string): any {
  let current: any = program.getGlobalNamespaceType();
  for (const segment of path.split(".")) {
    const member =
      current.namespaces?.get(segment) ??
      current.models?.get(segment) ??
      current.scalars?.get(segment) ??
      current.enums?.get(segment) ??
      current.unions?.get(segment) ??
      current.interfaces?.get(segment) ??
      current.operations?.get(segment) ??
      current.properties?.get(segment) ??
      current.members?.get(segment) ??
      current.variants?.get(segment);
    if (!member) throw new Error(`Cannot find ${segment} in ${path}`);
    current = member;
  }
  return current;
}

type Getter = (program: any, path: string) => unknown;

async function expectSameGetters(source: string, getters: Array<[string, Getter, string]>) {
  const { ts, rust } = await expectSameDiagnostics(source);
  for (const [name, getter, path] of getters) {
    const expected = describe_(getter(ts.program, path));
    expect(describe_(getter(rust.program, path)), `${name}(${path})`).toEqual(expected);
  }
}

const c = compilerLib as any;
const on =
  (fn: (program: any, type: any) => unknown): Getter =>
  (program, path) =>
    fn(program, find(program, path));

describe("std decorator diagnostics: Rust vs TS", () => {
  const cases: Array<[string, string]> = [
    ["wrong argument type", `@doc(123) model A {}`],
    ["too few arguments", `@doc() model A {}`],
    ["too many arguments", `@minLength(1, 2) scalar S extends string;`],
    ["decorator-wrong-target", `@minLength(1) model A {}`],
    ["decorator-wrong-target on property type", `model A { @minLength(1) x: int32 }`],
    ["wrong target with valid arguments elsewhere", `@format("x") op foo(): void;`],
    ["secret on model", `@secret model A {}`],
    ["minItems on non-array", `model A { @minItems(1) x: string }`],
    ["invalid range", `model A { @minValue(10) @maxValue(5) x: int32 }`],
    ["invalid length range", `model A { @minLength(10) @maxLength(5) x: string }`],
    ["numeric value too large for int32", `model A { @minLength(99999999999) x: string }`],
    ["invalid encode", `model A { @encode("rfc7231") x: int32 }`],
    ["encode string on boolean (85ff8283c)", `model A { @encode(string) x: boolean }`],
    ["encode bytes", `model A { @encode("base64url") x: bytes }`],
    ["encode duration", `model A { @encode("seconds", int32) x: duration }`],
    ["encode duration wrong type", `model A { @encode("seconds", string) x: duration }`],
    ["discriminator property type", `@discriminator("kind") model A { kind: int32 }`],
    [
      "discriminator ok",
      `@discriminator("kind") model A { kind: string } model B extends A { kind: "b" }`,
    ],
    ["key on optional", `model A { @key x?: string }`],
    ["duplicate key", `model A { @key x: string; @key y: string }`],
    ["visibility enum values", `model A { @visibility(Lifecycle.Read) x: string }`],
    ["visibility wrong value", `model A { @visibility("read") x: string }`],
    ["pattern", `@pattern("^a+$") scalar S extends string;`],
    ["service object value", `@service(#{ title: "My service" }) namespace N;`],
    ["error model", `@error model E { code: int32 }`],
    ["tags", `@tag("a") @tag("b") op foo(): void;`],
    ["overload", `op foo(x: string | int32): void; @overload(foo) op fooString(x: string): void;`],
    [
      "friendlyName",
      `@friendlyName("{name}Item", T) model Foo<T> { t: T } model A { x: Foo<string> }`,
    ],
    [
      "encodedName on enum member (5a18007e6)",
      `enum E { @encodedName("application/json", "A") a }`,
    ],
    [
      "encodedName duplicate",
      `model A { @encodedName("application/json", "y") x: string; y: string }`,
    ],
    ["mediaTypeHint", `@mediaTypeHint("text/plain") scalar S extends string;`],
    ["mediaTypeHint invalid", `@mediaTypeHint("not a mime") scalar S extends string;`],
    ["examples", `@example(#{ x: "a" }) model A { x: string }`],
    ["op examples", `@opExample(#{ parameters: #{ x: "a" } }) op foo(x: string): void;`],
    ["withOptionalProperties", `model A { x: string } model B { ...OptionalProperties<A> }`],
    ["augment decorator", `model A {} @@doc(A, "doc")`],
    ["augment unknown target", `@@doc(Missing, "doc")`],
    ["augment with wrong arguments", `model A {} @@doc(A, 1)`],
    ["missing-implementation", `extern dec foo(target: unknown);`],
    ["missing function implementation", `extern fn bar(): unknown;`],
    ["user decorator without extern", `dec foo(target: unknown);`],
    ["unknown decorator", `@notADecorator model A {}`],
    [
      "decorator on template instance target",
      `model T<X> { @doc("x") x: X } model A { t: T<string> }`,
    ],
    ["returnsDoc / errorsDoc", `@returnsDoc("ok") @errorsDoc("bad") op foo(): void;`],
    ["doc comments", `/** A model\n * @prop x the x */ model A { x: string }`],
    ["summary", `@summary("short") model A {}`],
    ["deprecated directive", `#deprecated "old"\nmodel A {} model B { a: A }`],
    [
      "deprecated references",
      `#deprecated "old scalar"\nscalar S extends string;\nscalar T extends S;\n#deprecated "old enum"\nenum E { a }\nmodel M { s: S; t: T; e: E; arr: S[]; u: S | int32 }\nop o(x: S): E;\nalias X = S;`,
    ],
    [
      "deprecated in deprecated context",
      `#deprecated "old"\nmodel A {}\n#deprecated "m"\nmodel B { a: A; nested: { a: A } }\n#deprecated "op"\nop o(a: A): A;\n#deprecated "i"\ninterface I { op p(a: A): void; }\n#deprecated "u"\nunion U { a: A }`,
    ],
    [
      "deprecated types inside deprecated operations (a06a62d03)",
      `#deprecated "old"\nmodel A {}\ninterface I {\n  #deprecated "op"\n  op p(a: A): A;\n  op q(a: A): void;\n}`,
    ],
    [
      "deprecated base model",
      `#deprecated "base"\nmodel A {}\nmodel B extends A {}\nmodel C { b: B }`,
    ],
    [
      "deprecated member",
      `model A { #deprecated "prop"\nx: string }\nenum E { #deprecated "member"\na }\nmodel B { x: A.x; e: E.a }`,
    ],
    ["deprecated template", `#deprecated "tpl"\nmodel T<X> { x: X }\nmodel A { t: T<string> }`],
    [
      "deprecated decorator",
      `#deprecated "dec"\nextern dec myDec(target: unknown);\n@myDec model A {}\n@@myDec(A)`,
    ],
    [
      "deprecated stdlib decorator",
      `@withVisibilityFilter(#{ all: #[Lifecycle.Read] }) model A { x: string }`,
    ],
    // TS crashes when a reference reaches a `#deprecated` directive with a missing or invalid argument.
    [
      "deprecated invalid arguments",
      `#deprecated\nmodel A {}\n#deprecated foo\nmodel B {}\n#deprecated "a"\n#deprecated "b"\nmodel C {}\nmodel D { c: C }`,
    ],
    ["qualified decorator reference", `@TypeSpec.doc("x") model A {}`],
    ["paging decorators", `model P { @pageItems items: string[] } @list op list(): P;`],
    ["paging invalid", `@list op list(): string;`],
  ];
  it.each(cases)("%s", async (_, source) => {
    await expectSameDiagnostics(source);
  });
});

describe("std decorator getters: Rust vs TS", () => {
  it("doc, summary and doc comments", async () => {
    await expectSameGetters(
      `/** Model doc */ @summary("short") model A { /** prop doc */ x: string; @doc("explicit {name}", { name: "n" }) y: string }
       @@doc(A.x, "augmented")
       /** op doc\n * @returns the result\n * @errors failure */ op foo(): void;`,
      [
        ["getDoc", on(c.getDoc), "A"],
        ["getSummary", on(c.getSummary), "A"],
        ["getDoc", on(c.getDoc), "A.x"],
        ["getDoc", on(c.getDoc), "A.y"],
        ["getDoc", on(c.getDoc), "foo"],
        ["getReturnsDoc", on(c.getReturnsDoc), "foo"],
        ["getErrorsDoc", on(c.getErrorsDoc), "foo"],
      ],
    );
  });

  it("constraints", async () => {
    await expectSameGetters(
      `@minLength(2) @maxLength(10) @pattern("^a", "must start with a") @format("custom") scalar S extends string;
       model A {
         @minValue(1) @maxValue(100) a: int32;
         @minValueExclusive(0.5) @maxValueExclusive(9.5) b: float64;
         @minItems(1) @maxItems(3) c: string[];
         @minValue(1) d: int64;
         @secret e: string;
       }`,
      [
        ["getMinLength", on(c.getMinLength), "S"],
        ["getMaxLength", on(c.getMaxLength), "S"],
        ["getPattern", on(c.getPattern), "S"],
        ["getPatternData", on(c.getPatternData), "S"],
        ["getFormat", on(c.getFormat), "S"],
        ["getMinValue", on(c.getMinValue), "A.a"],
        ["getMaxValue", on(c.getMaxValue), "A.a"],
        ["getMinValueExclusive", on(c.getMinValueExclusive), "A.b"],
        ["getMaxValueExclusive", on(c.getMaxValueExclusive), "A.b"],
        ["getMinItems", on(c.getMinItems), "A.c"],
        ["getMaxItems", on(c.getMaxItems), "A.c"],
        ["getMinValueAsNumeric", on(c.getMinValueAsNumeric), "A.d"],
        ["isSecret", on(c.isSecret), "A.e"],
        ["isSecret", on(c.isSecret), "A.a"],
      ],
    );
  });

  it("encode and encodedName", async () => {
    await expectSameGetters(
      `model A {
         @encode("rfc7231") a: utcDateTime;
         @encode(DateTimeKnownEncoding.unixTimestamp, int32) b: utcDateTime;
         @encode(string) c: boolean;
         @encode(BytesKnownEncoding.base64url) d: bytes;
         @encodedName("application/json", "ee") e: string;
       }
       enum E { @encodedName("application/json", "A_VALUE") a }`,
      [
        ["getEncode", on(c.getEncode), "A.a"],
        ["getEncode", on(c.getEncode), "A.b"],
        ["getEncode", on(c.getEncode), "A.c"],
        ["getEncode", on(c.getEncode), "A.d"],
        [
          "resolveEncodedName",
          (p, path) => c.resolveEncodedName(p, find(p, path), "application/json"),
          "A.e",
        ],
        [
          "resolveEncodedName",
          (p, path) => c.resolveEncodedName(p, find(p, path), "application/json"),
          "E.a",
        ],
        [
          "resolveEncodedName",
          (p, path) => c.resolveEncodedName(p, find(p, path), "application/xml"),
          "A.e",
        ],
      ],
    );
  });

  it("model metadata", async () => {
    await expectSameGetters(
      `@error model Err { code: int32 }
       @discriminator("kind") model Pet { kind: string }
       model Dog extends Pet { kind: "dog" }
       model K { @key id: string; @key("customName") other: string }
       @friendlyName("Renamed") model F {}
       @mediaTypeHint("application/xml") scalar X extends string;
       @withOptionalProperties model O { x: string }`,
      [
        ["isErrorModel", on(c.isErrorModel), "Err"],
        ["isErrorModel", on(c.isErrorModel), "Pet"],
        ["getDiscriminator", on(c.getDiscriminator), "Pet"],
        ["isKey", on(c.isKey), "K.id"],
        ["getKeyName", on(c.getKeyName), "K.id"],
        ["getKeyName", on(c.getKeyName), "K.other"],
        ["getFriendlyName", on(c.getFriendlyName), "F"],
        ["getMediaTypeHint", on(c.getMediaTypeHint), "X"],
      ],
    );
  });

  it("service, tags, overloads and examples", async () => {
    await expectSameGetters(
      `@service(#{ title: "Pet store" }) namespace PetStore {
         @tag("pets") @tag("read") op list(): void;
         op get(id: string | int32): void;
         @overload(get) op getByString(id: string): void;
         @example(#{ name: "rex", age: 3 }, #{ title: "A pet" }) model Pet { name: string; age: int32 }
         @opExample(#{ parameters: #{ id: "a" } }, #{ title: "ex" }) op one(id: string): void;
       }`,
      [
        ["getService", on(c.getService), "PetStore"],
        ["isService", on(c.isService), "PetStore"],
        ["getTags", on(c.getTags), "PetStore.list"],
        ["getOverloads", on(c.getOverloads), "PetStore.get"],
        ["getOverloadedOperation", on(c.getOverloadedOperation), "PetStore.getByString"],
        ["getExamples", on(c.getExamples), "PetStore.Pet"],
        ["getOpExamples", on(c.getOpExamples), "PetStore.one"],
      ],
    );
  });

  it("deprecation and visibility", async () => {
    await expectSameGetters(
      `#deprecated "use B"
       model A {}
       model B { @visibility(Lifecycle.Read) x: string; @invisible(Lifecycle) y: string }`,
      [
        ["getDeprecationDetails", on(c.getDeprecationDetails), "A"],
        ["isDeprecated", on(c.isDeprecated), "A"],
        [
          "isVisible(Read)",
          (p, path) =>
            c.isVisible(p, find(p, path), c.VisibilityFilter.fromArgs?.({}) ?? { all: new Set() }),
          "B.x",
        ],
      ],
    );
  });
});

describe("http decorators: Rust vs TS", () => {
  const h = httpLib as any;
  const http = (body: string) => `import "@typespec/http"; using Http;\n${body}`;

  it("operations and parameters", async () => {
    await expectSameGetters(
      http(`
        @route("/pets") namespace Pets {
          @get op list(@query filter?: string, @header("x-ms-id") id: string): void;
          @post op create(@path name: string, @body pet: Pet): void;
          @put @route("{id}") op update(@path id: string, @bodyRoot pet: Pet): void;
          @delete @sharedRoute @route("/x") op remove(): void;
        }
        model Pet { @visibility(Lifecycle.Read) id: string; name: string; @bodyIgnore meta: string }`),
      [
        ["getOperationVerb", on(h.getOperationVerb), "Pets.list"],
        ["getOperationVerb", on(h.getOperationVerb), "Pets.create"],
        ["getOperationVerb", on(h.getOperationVerb), "Pets.remove"],
        ["getRoutePath", on(h.getRoutePath), "Pets"],
        ["getRoutePath", on(h.getRoutePath), "Pets.update"],
        ["isSharedRoute", on(h.isSharedRoute), "Pets.remove"],
        ["isSharedRoute", on(h.isSharedRoute), "Pets.list"],
        [
          "getQueryParamName",
          (p, path) => h.getQueryParamName(p, find(p, path).parameters.properties.get("filter")),
          "Pets.list",
        ],
        [
          "getHeaderFieldName",
          (p, path) => h.getHeaderFieldName(p, find(p, path).parameters.properties.get("id")),
          "Pets.list",
        ],
        [
          "getPathParamName",
          (p, path) => h.getPathParamName(p, find(p, path).parameters.properties.get("name")),
          "Pets.create",
        ],
        [
          "isBody",
          (p, path) => h.isBody(p, find(p, path).parameters.properties.get("pet")),
          "Pets.create",
        ],
        [
          "isBodyRoot",
          (p, path) => h.isBodyRoot(p, find(p, path).parameters.properties.get("pet")),
          "Pets.update",
        ],
        ["isBodyIgnore", on(h.isBodyIgnore), "Pet.meta"],
      ],
    );
  });

  it("responses, servers and auth", async () => {
    await expectSameGetters(
      http(`
        @server("https://example.com/{region}", "Main", { region: string })
        @useAuth(BearerAuth | ApiKeyAuth<ApiKeyLocation.header, "x-key">)
        @service namespace S {
          model Ok { @statusCode code: 200 | 201; @header("x-next") next?: string }
          @error model Err { @statusCode code: 400; message: string }
          op get(): Ok | Err;
          @patch(#{ implicitOptionality: true }) op change(@body body: Ok): void;
          @head op exists(): void;
        }`),
      [
        ["getServers", on(h.getServers), "S"],
        ["getAuthentication", on(h.getAuthentication), "S"],
        ["isStatusCode", on(h.isStatusCode), "S.Ok.code"],
        ["getStatusCodes", on(h.getStatusCodes), "S.Ok.code"],
        ["getStatusCodes", on(h.getStatusCodes), "S.Err.code"],
        ["isHeader", on(h.isHeader), "S.Ok.next"],
        ["getOperationVerb", on(h.getOperationVerb), "S.change"],
        ["getPatchOptions", on(h.getPatchOptions), "S.change"],
        ["getOperationVerb", on(h.getOperationVerb), "S.exists"],
      ],
    );
  });

  const cases: Array<[string, string]> = [
    ["duplicate verb", `@get @post op foo(): void;`],
    ["status code out of range", `model R { @statusCode code: 999 }`],
    ["header on non-property", `@header model A {}`],
    ["path on optional without default", `op foo(@path id?: string): void;`],
    ["invalid server url", `@server("not a url {x}", "x") @service namespace S;`],
    ["route on model", `@route("/a") model A {}`],
    [
      "multipart body",
      `op upload(@header contentType: "multipart/form-data", @multipartBody body: { name: HttpPart<string> }): void;`,
    ],
  ];
  it.each(cases)("%s", async (_, source) => {
    await expectSameDiagnostics(http(source));
  });
});
