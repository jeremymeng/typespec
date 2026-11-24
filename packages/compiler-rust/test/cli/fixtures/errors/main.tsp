import "@typespec/json-schema";
using TypeSpec.JsonSchema;

@jsonSchema
namespace Errors;

model Pet {
  owner: Missing;
  #suppress "invalid-ref" "errors cannot be suppressed"
  friend: AlsoMissing;
}

@minLength("one")
scalar Name extends string;
