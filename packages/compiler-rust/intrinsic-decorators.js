export function createDecoratorRegistry(entries = {}) {
  return { ...entries };
}

export function registerDecorator(registry, name, fn) {
  registry[name] = fn;
  return registry;
}

export function createIntrinsicDecoratorRegistry() {
  return createDecoratorRegistry({
    "TypeSpec.@doc": docDecorator,
    "TypeSpec.@summary": summaryDecorator,
    "TypeSpec.@deprecated": deprecatedDecorator,
    "TypeSpec.@key": keyDecorator,
  });
}

export const intrinsicDecorators = createIntrinsicDecoratorRegistry();

function validateCount(context, decorator, args, min, max = min) {
  let count = args.length;
  while (count > 0 && args[count - 1] === undefined) count--;
  if (count < min || count > max) {
    context.reportDiagnostic({
      code: "invalid-argument-count",
      message: `Decorator ${decorator} expects ${min === max ? min : `${min}-${max}`} arguments.`,
    });
    return false;
  }
  return true;
}

function validateString(context, decorator, value) {
  if (typeof value !== "string") {
    throw new Error(`Decorator ${decorator} expected a string argument.`);
  }
  return true;
}

function docDecorator(context, target, docString) {
  if (!validateCount(context, "@doc", [docString], 1)) return;
  validateString(context, "@doc", docString);
  context.setProperty(target, "doc", docString);
}

function summaryDecorator(context, target, summary) {
  if (!validateCount(context, "@summary", [summary], 1)) return;
  validateString(context, "@summary", summary);
  context.setProperty(target, "summary", summary);
}

function deprecatedDecorator(context, target, message) {
  if (!validateCount(context, "@deprecated", [message], 1)) return;
  validateString(context, "@deprecated", message);
  context.setProperty(target, "deprecated", message);
}

function keyDecorator(context, target, altName) {
  if (!validateCount(context, "@key", [altName], 0, 1)) return;
  if (context.getKind(target) !== "ModelProperty") {
    context.reportDiagnostic({
      code: "decorator-wrong-target",
      message: "Decorator @key can only be applied to model properties.",
      target,
    });
    return;
  }
  if (altName !== undefined && altName !== null && typeof altName !== "string") {
    throw new Error("Decorator @key expected a string argument.");
  }
  context.setProperty(target, "key", altName ?? context.getName(target));
}
