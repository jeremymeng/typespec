import "@typespec/json-schema";
using TypeSpec.JsonSchema;

@jsonSchema
namespace Features;

union Base {
  a: string,
}

union Derived extends Base {
  b: "b",
}

model Holder {
  value: Derived;
}
