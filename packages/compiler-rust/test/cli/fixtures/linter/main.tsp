import "@typespec/json-schema";
using TypeSpec.JsonSchema;
using Linted.Helpers;

@jsonSchema
namespace Linted;

namespace Helpers {
  model Helper {}
}

model Box<T, Unused> {
  value: T;
}

model Pet {
  box: Box<string, int32>;
}
