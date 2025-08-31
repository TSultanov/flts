import { createEvolu, getOrThrow, SimpleName, type Evolu } from "@evolu/common";
import { evoluSvelteDeps } from "@evolu/svelte";
import { Schema, type DatabaseSchema } from "./schema";

const evolu: Evolu<DatabaseSchema> = createEvolu(evoluSvelteDeps)(Schema, {
    name: getOrThrow(SimpleName.from("flts"))
});