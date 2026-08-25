export const greeting: string = "hello";

export function add(a: number, b: number): number {
  return a + b;
}

export interface Point {
  x: number;
  y: number;
}

export class Vec implements Point {
  constructor(
    public x: number,
    public y: number,
  ) {}

  length(): number {
    return Math.sqrt(this.x * this.x + this.y * this.y);
  }
}
