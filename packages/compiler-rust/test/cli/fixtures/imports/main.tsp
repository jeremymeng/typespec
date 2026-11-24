import "@typespec/json-schema";
import "#models/pet.tsp";
using TypeSpec.JsonSchema;

@jsonSchema
namespace Imports;

model Owner {
  pet: Models.Pet;
}
