import { double } from "./util.js";
import type { Point } from "./util.js";

export type Doubler = typeof double;
export const origin: Point = { x: 0, y: 0 };
