import "@typespec/json-schema";
using TypeSpec.JsonSchema;

@jsonSchema
namespace Warnings;

#deprecated "Use NewPet"
model OldPet {
  name: string;
}

model Owner {
  pet: OldPet;
  #suppress "deprecated" "Kept for compatibility"
  legacy: OldPet;
  #suppress "deprecated" "first"
  #suppress "deprecated" "second"
  other: OldPet;
}
