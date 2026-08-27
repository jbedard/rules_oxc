export declare const greeting: string;
export declare function add(a: number, b: number): number;
export interface Point {
	x: number;
	y: number;
}
export declare class Vec implements Point {
	x: number;
	y: number;
	constructor(x: number, y: number);
	length(): number;
}
