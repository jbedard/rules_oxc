import { local } from "./local";
import { shared } from "./shared";
import { answer } from "./gen";
import { generated } from "./genmod";

export const total: number = local + shared + answer + generated;
