import "./sub/index";
import { double } from "./util";
import type { Point } from "./main";
import { type Vec, greeting } from "./main";

export type Doubler = typeof double;
export type Shape = Vec;
export type { Point };
export { type Point as Coord, greeting };
export const origin: Point = { x: 0, y: 0 };
