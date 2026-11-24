import "@typespec/json-schema";
using TypeSpec.JsonSchema;

@jsonSchema
namespace Entrypoint;

model Widget {
  id: string;
}
