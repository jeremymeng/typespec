import "@typespec/json-schema";
using TypeSpec.JsonSchema;

/** Pet store schemas. */
@jsonSchema
namespace PetStore;

/** A pet. */
model Pet {
  /** The pet's name. */
  @minLength(1)
  name: string;

  age?: int32;
  kind: PetKind;
  tags: string[];
  owner?: Owner;
}

enum PetKind {
  dog,
  cat,
}

model Owner {
  @format("email")
  email: string;

  pets: Pet[];
}

union IdOrName {
  id: int64,
  name: string,
}
