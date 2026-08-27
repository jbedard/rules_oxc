import { local } from "./local.js";
import { shared } from "./shared";
import { answer } from "./gen";
import { generated } from "./genmod";
export const total = local + shared + answer + generated;
