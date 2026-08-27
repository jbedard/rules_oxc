import { local } from "./local.js";
import { shared } from "./shared.js";
import { answer } from "./gen.js";
import { generated } from "./genmod.js";
export const total = local + shared + answer + generated;
